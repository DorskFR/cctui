use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;

use cctui_proto::api::{
    ApiError, AttachLabelRequest, CreateLabelRequest, Label, LabelListResponse, UpdateLabelRequest,
};

use crate::state::AppState;

// --- Session labels (CCT-360) ---
//
// Label *definitions* (list/create/update/delete below) are a global, shared
// vocabulary: they carry no per-user data (just name + color) and are not owned
// by any user, so requiring authentication (the `auth_middleware` all these
// routes sit behind) is sufficient — there is no cross-user data to leak here.
// The per-session attach/detach routes, by contrast, ARE ownership-gated via
// `authorize_session` since they mutate a specific session (CCT-417).

fn db_err(e: &sqlx::Error) -> (StatusCode, Json<ApiError>) {
    tracing::error!("db error: {e}");
    (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "database error".into() }))
}

/// `GET /api/v1/labels` — every label known to the server, ordered most-recently
/// used (or created, whichever is later) first so the picker can surface the
/// handful you actually reach for without listing them all. Feeds both the
/// per-session label picker and the sessions-page filter.
pub async fn list_labels(
    State(state): State<AppState>,
) -> Result<Json<LabelListResponse>, (StatusCode, Json<ApiError>)> {
    let rows: Vec<(uuid::Uuid, String, String)> = sqlx::query_as(
        "SELECT l.id, l.name, l.color \
         FROM labels l \
         LEFT JOIN session_labels sl ON sl.label_id = l.id \
         GROUP BY l.id, l.name, l.color, l.created_at \
         ORDER BY GREATEST(l.created_at, COALESCE(MAX(sl.created_at), l.created_at)) DESC, \
                  lower(l.name)",
    )
    .fetch_all(&state.pool)
    .await
    .map_err(|e| db_err(&e))?;
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
    Json(req): Json<CreateLabelRequest>,
) -> Result<(StatusCode, Json<Label>), (StatusCode, Json<ApiError>)> {
    let name = req.name.trim();
    if name.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ApiError { error: "label name is required".into() }),
        ));
    }
    let row: (uuid::Uuid, String, String) = sqlx::query_as(
        "INSERT INTO labels (name, color) VALUES ($1, $2) \
         ON CONFLICT (lower(name)) DO UPDATE SET color = EXCLUDED.color \
         RETURNING id, name, color",
    )
    .bind(name)
    .bind(&req.color)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| db_err(&e))?;
    Ok((StatusCode::CREATED, Json(Label { id: row.0.to_string(), name: row.1, color: row.2 })))
}

/// `PATCH /api/v1/labels/{id}` — rename and/or recolor an existing label by id.
/// Unlike the POST get-or-create (which is keyed on name), this edits a specific
/// label in place so the picker's edit dialog can rename without orphaning the
/// old name. A rename that collides with another label's (case-insensitive) name
/// is rejected with 409.
pub async fn update_label(
    State(state): State<AppState>,
    Path(label_id): Path<String>,
    Json(req): Json<UpdateLabelRequest>,
) -> Result<Json<Label>, (StatusCode, Json<ApiError>)> {
    let id = parse_label_id(&label_id)?;
    let name = match req.name.as_deref().map(str::trim) {
        Some("") => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ApiError { error: "label name is required".into() }),
            ));
        }
        other => other,
    };
    let row: Option<(uuid::Uuid, String, String)> = sqlx::query_as(
        "UPDATE labels SET \
             name = COALESCE($2, name), \
             color = COALESCE($3, color) \
         WHERE id = $1 \
         RETURNING id, name, color",
    )
    .bind(id)
    .bind(name)
    .bind(req.color.as_deref())
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| {
        // Unique violation on labels_name_lower_key → name collides with another.
        if let sqlx::Error::Database(dbe) = &e
            && dbe.code().as_deref() == Some("23505")
        {
            return (
                StatusCode::CONFLICT,
                Json(ApiError { error: "a label with that name already exists".into() }),
            );
        }
        db_err(&e)
    })?;
    match row {
        Some(r) => Ok(Json(Label { id: r.0.to_string(), name: r.1, color: r.2 })),
        None => Err((StatusCode::NOT_FOUND, Json(ApiError { error: "label not found".into() }))),
    }
}

/// `DELETE /api/v1/labels/{id}` — delete a label globally; cascades to detach
/// it from every session.
pub async fn delete_label(
    State(state): State<AppState>,
    Path(label_id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    let id = parse_label_id(&label_id)?;
    sqlx::query("DELETE FROM labels WHERE id = $1")
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(|e| db_err(&e))?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/sessions/{id}/labels` — attach an existing label to a session.
/// Idempotent (re-attaching the same label is a no-op).
pub async fn attach_label(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(req): Json<AttachLabelRequest>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    let label_id = parse_label_id(&req.label_id)?;
    sqlx::query(
        "INSERT INTO session_labels (session_id, label_id) VALUES ($1, $2) \
         ON CONFLICT DO NOTHING",
    )
    .bind(&session_id)
    .bind(label_id)
    .execute(&state.pool)
    .await
    .map_err(|e| db_err(&e))?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/v1/sessions/{id}/labels/{label_id}` — detach a label from a
/// session (leaves the label definition intact for other sessions).
pub async fn detach_label(
    State(state): State<AppState>,
    Path((session_id, label_id)): Path<(String, String)>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    let label_id = parse_label_id(&label_id)?;
    sqlx::query("DELETE FROM session_labels WHERE session_id = $1 AND label_id = $2")
        .bind(&session_id)
        .bind(label_id)
        .execute(&state.pool)
        .await
        .map_err(|e| db_err(&e))?;
    Ok(StatusCode::NO_CONTENT)
}

fn parse_label_id(raw: &str) -> Result<uuid::Uuid, (StatusCode, Json<ApiError>)> {
    uuid::Uuid::parse_str(raw)
        .map_err(|_| (StatusCode::BAD_REQUEST, Json(ApiError { error: "invalid label id".into() })))
}
