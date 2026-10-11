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
///
/// The label joins the caller's vocabulary. An admin sees every user's labels
/// and may name the session or machine the label is meant for: it then joins
/// that owner's vocabulary, the one a session's labels always live in.
pub async fn create_label(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<CreateLabelRequest>,
) -> Result<(StatusCode, Json<Label>), AppError> {
    let name = req.name.trim();
    if name.is_empty() {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "label name is required"));
    }
    let owner = match ctx.owner_filter() {
        Some(uid) => uid,
        None => admin_label_owner(&state, &req).await?.unwrap_or(ctx.user_id),
    };
    let row: (uuid::Uuid, String, String) = sqlx::query_as(
        "INSERT INTO labels (name, color, user_id) VALUES ($1, $2, $3) \
         ON CONFLICT (user_id, lower(name)) DO UPDATE SET color = EXCLUDED.color \
         RETURNING id, name, color",
    )
    .bind(name)
    .bind(&req.color)
    .bind(owner)
    .fetch_one(&state.pool)
    .await?;
    Ok((StatusCode::CREATED, Json(Label { id: row.0.to_string(), name: row.1, color: row.2 })))
}

/// The owner of the session, else of the machine, an admin's label request
/// names. `None` when it names neither (or neither resolves).
async fn admin_label_owner(
    state: &AppState,
    req: &CreateLabelRequest,
) -> Result<Option<uuid::Uuid>, AppError> {
    let mut conn = state.pool.acquire().await?;
    if let Some(sid) = req.session_id.as_deref().filter(|s| !s.is_empty())
        && let Some(owner) = crate::spawn_labels::session_owner(&mut conn, sid).await?
    {
        return Ok(Some(owner));
    }
    let Some(machine) = req.machine_id.as_deref().and_then(|m| uuid::Uuid::parse_str(m).ok())
    else {
        return Ok(None);
    };
    Ok(sqlx::query_scalar("SELECT user_id FROM machines WHERE id = $1")
        .bind(machine)
        .fetch_optional(&mut *conn)
        .await?)
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
/// Idempotent (re-attaching the same label is a no-op). The label must be one
/// the caller sees; another user's label (an admin's pick, or the caller's own
/// on a session it may write but does not own) lands as the session owner's
/// label of the same name, so a session only ever carries its owner's labels.
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
    let mut tx = state.pool.begin().await?;
    crate::spawn_labels::attach_to_session(&mut tx, &session_id, &[label_id]).await?;
    tx.commit().await?;
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

    fn admin(user_id: Uuid) -> AuthContext {
        AuthContext { scopes: [Scope::Read, Scope::Admin].into_iter().collect(), ..caller(user_id) }
    }

    /// A machine and an active session on it, both owned by `uid`.
    async fn owned_session(pool: &sqlx::PgPool, uid: Uuid) -> (Uuid, String) {
        let machine = Uuid::new_v4();
        let sid = Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, 'm', $3)")
            .bind(machine)
            .bind(uid)
            .bind(format!("mklab-{machine}"))
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status) \
             VALUES ($1, $2, $2, $3, '/w', 'active')",
        )
        .bind(&sid)
        .bind(machine)
        .bind(uid)
        .execute(pool)
        .await
        .unwrap();
        (machine, sid)
    }

    async fn owner_of(pool: &sqlx::PgPool, label_id: &str) -> Uuid {
        sqlx::query_scalar("SELECT user_id FROM labels WHERE id = $1")
            .bind(Uuid::parse_str(label_id).unwrap())
            .fetch_one(pool)
            .await
            .unwrap()
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
            Json(CreateLabelRequest {
                name: name.clone(),
                color: "#abcdef".into(),
                session_id: None,
                machine_id: None,
            }),
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
            Json(CreateLabelRequest {
                name,
                color: "#123456".into(),
                session_id: None,
                machine_id: None,
            }),
        )
        .await
        .unwrap();
        assert_ne!(copy.id, label.id);

        let Json(mine_listed) = list_labels(State(state), Extension(caller(mine))).await.unwrap();
        assert!(mine_listed.labels.iter().any(|l| l.id == label.id && l.color == "#abcdef"));
    }

    /// An admin's label lands where the session's labels live: the owner of
    /// the session or machine it names, else the admin itself. A non-admin's
    /// hint is ignored, its labels are always its own.
    #[tokio::test]
    async fn admin_label_joins_the_named_owner() {
        let Some(pool) = test_pool("admin_label_joins_the_named_owner").await else {
            return;
        };
        let (root, owner, other) = (user(&pool).await, user(&pool).await, user(&pool).await);
        let (machine, sid) = owned_session(&pool, owner).await;
        let state = AppState::for_test(pool.clone());
        let create = |ctx: AuthContext, session_id: Option<String>, machine_id: Option<String>| {
            let state = state.clone();
            async move {
                let (_, Json(label)) = create_label(
                    State(state),
                    Extension(ctx),
                    Json(CreateLabelRequest {
                        name: format!("hint-{}", Uuid::new_v4()),
                        color: "#abcdef".into(),
                        session_id,
                        machine_id,
                    }),
                )
                .await
                .unwrap();
                label.id
            }
        };

        let by_session = create(admin(root), Some(sid.clone()), None).await;
        assert_eq!(owner_of(&pool, &by_session).await, owner);
        let by_machine = create(admin(root), None, Some(machine.to_string())).await;
        assert_eq!(owner_of(&pool, &by_machine).await, owner);
        let unhinted = create(admin(root), None, None).await;
        assert_eq!(owner_of(&pool, &unhinted).await, root);
        let ignored = create(caller(other), Some(sid), Some(machine.to_string())).await;
        assert_eq!(owner_of(&pool, &ignored).await, other);
    }

    /// Attaching another user's label (an admin's pick) puts the session
    /// owner's label of the same name on the session, never the foreign row;
    /// a non-admin still cannot attach a label it does not see.
    #[tokio::test]
    async fn attach_maps_a_foreign_label_onto_the_owner() {
        let Some(pool) = test_pool("attach_maps_a_foreign_label_onto_the_owner").await else {
            return;
        };
        let (root, owner) = (user(&pool).await, user(&pool).await);
        let (_, sid) = owned_session(&pool, owner).await;
        let state = AppState::for_test(pool.clone());
        let name = format!("Pick-{}", Uuid::new_v4());
        let (_, Json(foreign)) = create_label(
            State(state.clone()),
            Extension(admin(root)),
            Json(CreateLabelRequest {
                name: name.clone(),
                color: "120".into(),
                session_id: None,
                machine_id: None,
            }),
        )
        .await
        .unwrap();
        assert_eq!(owner_of(&pool, &foreign.id).await, root);

        let denied = attach_label(
            State(state.clone()),
            Extension(caller(owner)),
            Path(sid.clone()),
            Json(AttachLabelRequest { label_id: foreign.id.clone() }),
        )
        .await;
        assert!(denied.is_err(), "a non-admin cannot attach a label it does not see");

        for _ in 0..2 {
            attach_label(
                State(state.clone()),
                Extension(admin(root)),
                Path(sid.clone()),
                Json(AttachLabelRequest { label_id: foreign.id.clone() }),
            )
            .await
            .unwrap();
        }
        let on_session: Vec<(Uuid, String, String, Uuid)> = sqlx::query_as(
            "SELECT l.id, l.name, l.color, l.user_id FROM session_labels sl \
               JOIN labels l ON l.id = sl.label_id WHERE sl.session_id = $1",
        )
        .bind(&sid)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(on_session.len(), 1, "attached once, as the owner's label");
        let (id, got_name, color, user_id) = &on_session[0];
        assert_eq!((got_name, color.as_str(), *user_id), (&name, "120", owner));
        assert_ne!(id.to_string(), foreign.id);
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
