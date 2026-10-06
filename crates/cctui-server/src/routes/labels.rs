use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};

use cctui_proto::api::{
    AttachLabelRequest, CreateLabelRequest, Label, LabelListResponse, UpdateLabelRequest,
};

use crate::auth::AuthContext;
use crate::error::AppError;
use crate::state::AppState;

// --- Session labels ---
//
// Label *definitions* (list/create/update/delete below) are per-user
// vocabulary: `labels.user_id` owns the row, names are unique per user, and
// every query here carries the caller's owner filter (NULL for an admin, the
// god-view). The per-session attach/detach routes are additionally
// ownership-gated on the session by the route's `Resource(Session, Write)`
// policy.

/// `GET /api/v1/labels` — the caller's labels, ordered most-recently used (or
/// created, whichever is later) first so the picker can surface the handful you
/// actually reach for without listing them all. Feeds both the per-session
/// label picker and the sessions-page filter.
pub async fn list_labels(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<LabelListResponse>, AppError> {
    let rows: Vec<(uuid::Uuid, String, String)> = sqlx::query_as(
        "SELECT l.id, l.name, l.color \
         FROM labels l \
         LEFT JOIN session_labels sl ON sl.label_id = l.id \
         WHERE $1::uuid IS NULL OR l.user_id = $1 \
         GROUP BY l.id, l.name, l.color, l.created_at \
         ORDER BY GREATEST(l.created_at, COALESCE(MAX(sl.created_at), l.created_at)) DESC, \
                  lower(l.name)",
    )
    .bind(ctx.owner_filter())
    .fetch_all(&state.pool)
    .await?;
    let labels = rows
        .into_iter()
        .map(|(id, name, color)| Label { id: id.to_string(), name, color })
        .collect();
    Ok(Json(LabelListResponse { labels }))
}

/// `POST /api/v1/labels` — get-or-create a label by case-insensitive name. If
/// the name already exists its color is refreshed to the supplied one (lets the
/// picker recolor a label); returns the resulting label either way.
pub async fn create_label(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<CreateLabelRequest>,
) -> Result<(StatusCode, Json<Label>), AppError> {
    let name = req.name.trim();
    if name.is_empty() {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "label name is required"));
    }
    let row: (uuid::Uuid, String, String) = sqlx::query_as(
        "INSERT INTO labels (name, color, user_id) VALUES ($1, $2, $3) \
         ON CONFLICT (user_id, lower(name)) DO UPDATE SET color = EXCLUDED.color \
         RETURNING id, name, color",
    )
    .bind(name)
    .bind(&req.color)
    .bind(ctx.user_id)
    .fetch_one(&state.pool)
    .await?;
    Ok((StatusCode::CREATED, Json(Label { id: row.0.to_string(), name: row.1, color: row.2 })))
}

/// `PATCH /api/v1/labels/{id}` — rename and/or recolor an existing label by id.
/// Unlike the POST get-or-create (which is keyed on name), this edits a specific
/// label in place so the picker's edit dialog can rename without orphaning the
/// old name. A rename that collides with another label's (case-insensitive) name
/// is rejected with 409.
pub async fn update_label(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(label_id): Path<String>,
    Json(req): Json<UpdateLabelRequest>,
) -> Result<Json<Label>, AppError> {
    let id = parse_label_id(&label_id)?;
    let name = match req.name.as_deref().map(str::trim) {
        Some("") => {
            return Err(AppError::new(StatusCode::BAD_REQUEST, "label name is required"));
        }
        other => other,
    };
    let row: Option<(uuid::Uuid, String, String)> = sqlx::query_as(
        "UPDATE labels SET \
             name = COALESCE($2, name), \
             color = COALESCE($3, color) \
         WHERE id = $1 AND ($4::uuid IS NULL OR user_id = $4) \
         RETURNING id, name, color",
    )
    .bind(id)
    .bind(name)
    .bind(req.color.as_deref())
    .bind(ctx.owner_filter())
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| {
        // Unique violation on labels_user_name_lower_key → name collides with another.
        if let sqlx::Error::Database(dbe) = &e
            && dbe.code().as_deref() == Some("23505")
        {
            return AppError::new(StatusCode::CONFLICT, "a label with that name already exists");
        }
        AppError::from(e)
    })?;
    match row {
        Some(r) => Ok(Json(Label { id: r.0.to_string(), name: r.1, color: r.2 })),
        None => Err(AppError::new(StatusCode::NOT_FOUND, "label not found")),
    }
}

/// `DELETE /api/v1/labels/{id}` — delete one of the caller's labels; cascades
/// to detach it from every session carrying it.
pub async fn delete_label(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(label_id): Path<String>,
) -> Result<StatusCode, AppError> {
    let id = parse_label_id(&label_id)?;
    sqlx::query("DELETE FROM labels WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)")
        .bind(id)
        .bind(ctx.owner_filter())
        .execute(&state.pool)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/sessions/{id}/labels` — attach an existing label to a session.
/// Idempotent (re-attaching the same label is a no-op).
pub async fn attach_label(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(session_id): Path<String>,
    Json(req): Json<AttachLabelRequest>,
) -> Result<StatusCode, AppError> {
    let label_id = parse_label_id(&req.label_id)?;
    let visible: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM labels \
                        WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2))",
    )
    .bind(label_id)
    .bind(ctx.owner_filter())
    .fetch_one(&state.pool)
    .await?;
    if !visible {
        return Err(AppError::new(StatusCode::NOT_FOUND, "label not found"));
    }
    sqlx::query(
        "INSERT INTO session_labels (session_id, label_id) VALUES ($1, $2) \
         ON CONFLICT DO NOTHING",
    )
    .bind(&session_id)
    .bind(label_id)
    .execute(&state.pool)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/v1/sessions/{id}/labels/{label_id}` — detach a label from a
/// session (leaves the label definition intact for other sessions).
pub async fn detach_label(
    State(state): State<AppState>,
    Path((session_id, label_id)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    let label_id = parse_label_id(&label_id)?;
    sqlx::query("DELETE FROM session_labels WHERE session_id = $1 AND label_id = $2")
        .bind(&session_id)
        .bind(label_id)
        .execute(&state.pool)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

fn parse_label_id(raw: &str) -> Result<uuid::Uuid, AppError> {
    uuid::Uuid::parse_str(raw)
        .map_err(|_| AppError::new(StatusCode::BAD_REQUEST, "invalid label id"))
}

#[cfg(test)]
mod tests {
    use axum::response::IntoResponse;
    use uuid::Uuid;

    use super::*;
    use crate::auth::Scope;

    fn caller(user_id: Uuid) -> AuthContext {
        AuthContext {
            user_id,
            key_id: Uuid::new_v4(),
            machine_id: None,
            scopes: std::iter::once(Scope::Read).collect(),
        }
    }

    async fn test_pool(test_name: &str) -> Option<sqlx::PgPool> {
        let url = crate::routes::gateway::test_db_url(test_name)?;
        Some(
            sqlx::postgres::PgPoolOptions::new()
                .max_connections(2)
                .connect(&url)
                .await
                .expect("connect test db"),
        )
    }

    async fn user(pool: &sqlx::PgPool) -> Uuid {
        let uid = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("label-{uid}"))
            .bind(format!("hlabel-{uid}"))
            .execute(pool)
            .await
            .unwrap();
        uid
    }

    /// Labels are per-user: a second tenant neither sees, renames nor deletes
    /// them, and the same name on both sides stays two distinct labels.
    #[tokio::test]
    async fn labels_are_not_visible_across_users() {
        let Some(pool) = test_pool("labels_are_not_visible_across_users").await else {
            return;
        };
        let (mine, theirs) = (user(&pool).await, user(&pool).await);
        let state = AppState::for_test(pool.clone());
        let name = format!("lane-{}", Uuid::new_v4());

        let (_, Json(label)) = create_label(
            State(state.clone()),
            Extension(caller(mine)),
            Json(CreateLabelRequest { name: name.clone(), color: "#abcdef".into() }),
        )
        .await
        .unwrap();

        let Json(listed) =
            list_labels(State(state.clone()), Extension(caller(theirs))).await.unwrap();
        assert!(
            !listed.labels.iter().any(|l| l.id == label.id),
            "another user's label must not be listed"
        );

        let renamed = update_label(
            State(state.clone()),
            Extension(caller(theirs)),
            Path(label.id.clone()),
            Json(UpdateLabelRequest { name: Some("stolen".into()), color: None }),
        )
        .await;
        assert!(renamed.is_err(), "another user's label must not be renameable");

        delete_label(State(state.clone()), Extension(caller(theirs)), Path(label.id.clone()))
            .await
            .unwrap();
        let still_there: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM labels WHERE id = $1)")
                .bind(Uuid::parse_str(&label.id).unwrap())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(still_there, "another user's delete must not remove the label");

        // The same name on another user is a new label, not an upsert of mine.
        let (_, Json(copy)) = create_label(
            State(state.clone()),
            Extension(caller(theirs)),
            Json(CreateLabelRequest { name, color: "#123456".into() }),
        )
        .await
        .unwrap();
        assert_ne!(copy.id, label.id);

        let Json(mine_listed) = list_labels(State(state), Extension(caller(mine))).await.unwrap();
        assert!(mine_listed.labels.iter().any(|l| l.id == label.id && l.color == "#abcdef"));
    }

    #[tokio::test]
    async fn db_failure_is_opaque_500() {
        let pool = sqlx::PgPool::connect_lazy("postgres://invalid").unwrap();
        pool.close().await;
        let resp = list_labels(State(AppState::for_test(pool)), Extension(caller(Uuid::new_v4())))
            .await
            .unwrap_err()
            .into_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(body, format!(r#"{{"error":"{}"}}"#, crate::error::DB_ERROR));
    }
}
