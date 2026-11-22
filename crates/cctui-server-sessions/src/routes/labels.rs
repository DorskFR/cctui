use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};

use cctui_proto::api::{
    AttachLabelRequest, CreateLabelRequest, Label, LabelListResponse, UpdateLabelRequest,
};

use crate::auth::AuthContext;
use crate::context::SessionsContext;
use crate::error::AppError;

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
/// actually reach for without listing them all.
///
/// Feeds both the per-session
/// label picker and the sessions-page filter.
pub async fn list_labels<S: SessionsContext>(
    State(state): State<S>,
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
    .fetch_all(state.pool())
    .await?;
    let labels = rows
        .into_iter()
        .map(|(id, name, color)| Label { id: id.to_string(), name, color })
        .collect();
    Ok(Json(LabelListResponse { labels }))
}

/// `POST /api/v1/labels` — get-or-create a label by case-insensitive name.
///
/// If
/// the name already exists its color is refreshed to the supplied one (lets the
/// picker recolor a label); returns the resulting label either way.
///
/// The label joins the caller's vocabulary. An admin sees every user's labels
/// and may name the session or machine the label is meant for: it then joins
/// that owner's vocabulary, the one a session's labels always live in.
pub async fn create_label<S: SessionsContext>(
    State(state): State<S>,
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
    .fetch_one(state.pool())
    .await?;
    Ok((StatusCode::CREATED, Json(Label { id: row.0.to_string(), name: row.1, color: row.2 })))
}

/// The owner of the session, else of the machine, an admin's label request
/// names. `None` when it names neither (or neither resolves).
async fn admin_label_owner<S: SessionsContext>(
    state: &S,
    req: &CreateLabelRequest,
) -> Result<Option<uuid::Uuid>, AppError> {
    let mut conn = state.pool().acquire().await?;
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
///
/// Unlike the POST get-or-create (which is keyed on name), this edits a specific
/// label in place so the picker's edit dialog can rename without orphaning the
/// old name. A rename that collides with another label's (case-insensitive) name
/// is rejected with 409.
pub async fn update_label<S: SessionsContext>(
    State(state): State<S>,
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
    .fetch_optional(state.pool())
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
pub async fn delete_label<S: SessionsContext>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
    Path(label_id): Path<String>,
) -> Result<StatusCode, AppError> {
    let id = parse_label_id(&label_id)?;
    sqlx::query("DELETE FROM labels WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)")
        .bind(id)
        .bind(ctx.owner_filter())
        .execute(state.pool())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/sessions/{id}/labels` — attach an existing label to a session.
///
/// Idempotent (re-attaching the same label is a no-op). The label must be one
/// the caller sees; another user's label (an admin's pick, or the caller's own
/// on a session it may write but does not own) lands as the session owner's
/// label of the same name, so a session only ever carries its owner's labels.
pub async fn attach_label<S: SessionsContext>(
    State(state): State<S>,
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
    .fetch_one(state.pool())
    .await?;
    if !visible {
        return Err(AppError::new(StatusCode::NOT_FOUND, "label not found"));
    }
    let mut tx = state.pool().begin().await?;
    crate::spawn_labels::attach_to_session(&mut tx, &session_id, &[label_id]).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/v1/sessions/{id}/labels/{label_id}` — detach a label from a
/// session (leaves the label definition intact for other sessions).
pub async fn detach_label<S: SessionsContext>(
    State(state): State<S>,
    Path((session_id, label_id)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    let label_id = parse_label_id(&label_id)?;
    sqlx::query("DELETE FROM session_labels WHERE session_id = $1 AND label_id = $2")
        .bind(&session_id)
        .bind(label_id)
        .execute(state.pool())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

fn parse_label_id(raw: &str) -> Result<uuid::Uuid, AppError> {
    uuid::Uuid::parse_str(raw)
        .map_err(|_| AppError::new(StatusCode::BAD_REQUEST, "invalid label id"))
}
