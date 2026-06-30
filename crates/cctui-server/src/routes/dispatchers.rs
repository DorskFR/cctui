//! Per-user enrolled-dispatcher management (CCT-285): `GET /api/v1/dispatchers`
//! (list with liveness), `PATCH /api/v1/dispatchers/{id}` (rename), and
//! `DELETE /api/v1/dispatchers/{id}` (remove). Peer of the machines management
//! surface.
//!
//! Enrollment itself (minting an identity + key) is `POST
//! /api/v1/dispatcher/enroll` ([`crate::routes::dispatcher`]); a dispatcher key
//! is returned once there and never echoed here.
//!
//! Auth: a user-scoped token operates on its own dispatchers; an admin token
//! (no owning user) may list across all users.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::auth::{AuthContext, Scope};
use crate::state::AppState;

#[derive(Debug, serde::Serialize)]
pub struct DispatcherInfo {
    pub id: Uuid,
    pub name: String,
    /// Reported by the binary at enroll: `kubernetes` | `docker` | `http`.
    pub kind: String,
    /// Non-secret fragment of the enrollment key, for display.
    pub key_preview: Option<String>,
    /// Liveness tier derived from `last_seen_at` age.
    pub liveness: cctui_proto::models::MachineLiveness,
    /// Whether a live WS connection is currently registered for this dispatcher.
    pub connected: bool,
    pub last_seen_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct DispatcherRow {
    id: Uuid,
    name: String,
    kind: String,
    key_preview: Option<String>,
    last_seen_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl DispatcherRow {
    fn into_info(self, state: &AppState) -> DispatcherInfo {
        let connected = state.dispatcher_connections.contains_key(&self.id);
        let liveness = crate::machine_liveness::derive(self.last_seen_at);
        DispatcherInfo {
            id: self.id,
            name: self.name,
            kind: self.kind,
            key_preview: self.key_preview,
            liveness,
            connected,
            last_seen_at: self.last_seen_at,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

#[derive(Debug, serde::Deserialize)]
pub struct RenameDispatcher {
    pub name: String,
}

fn db_err(e: &sqlx::Error) -> (StatusCode, Json<serde_json::Value>) {
    if let sqlx::Error::Database(dbe) = e
        && dbe.code().as_deref() == Some("23505")
    {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "error": "a dispatcher with that name already exists" })),
        );
    }
    tracing::error!("dispatchers db error: {e}");
    (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": "db error" })))
}

const SELECT_COLS: &str =
    "id, name, kind, key_preview, last_seen_at, created_at, updated_at FROM dispatchers";

pub async fn list_dispatchers(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<Vec<DispatcherInfo>>, (StatusCode, Json<serde_json::Value>)> {
    // Uniform god-view (CCT-410): admin (`owner_filter` = NULL) sees all
    // dispatchers; a user sees only their own.
    let rows: Vec<DispatcherRow> = sqlx::query_as(&format!(
        "SELECT {SELECT_COLS} \
         WHERE ($1::uuid IS NULL OR user_id = $1) \
         AND deleted_at IS NULL AND revoked_at IS NULL ORDER BY name"
    ))
    .bind(ctx.owner_filter())
    .fetch_all(&state.pool)
    .await
    .map_err(|e| db_err(&e))?;

    Ok(Json(rows.into_iter().map(|r| r.into_info(&state)).collect()))
}

pub async fn update_dispatcher(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    Json(req): Json<RenameDispatcher>,
) -> Result<Json<DispatcherInfo>, (StatusCode, Json<serde_json::Value>)> {
    ctx.requires(Scope::Enroll).map_err(|s| {
        (
            s,
            Json(
                serde_json::json!({ "error": "the enroll scope is required to edit dispatchers" }),
            ),
        )
    })?;
    if req.name.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "name is required" })),
        ));
    }

    let row: Option<DispatcherRow> = sqlx::query_as(
        "UPDATE dispatchers SET name = $3, updated_at = now() \
         WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2) AND deleted_at IS NULL \
         RETURNING id, name, kind, key_preview, last_seen_at, created_at, updated_at",
    )
    .bind(id)
    .bind(ctx.owner_filter())
    .bind(req.name.trim())
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| db_err(&e))?;

    row.map(|r| Json(r.into_info(&state))).ok_or_else(|| {
        (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "dispatcher not found" })))
    })
}

pub async fn delete_dispatcher(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    ctx.requires(Scope::Enroll).map_err(|s| {
        (s, Json(serde_json::json!({ "error": "the enroll scope is required to delete dispatchers" })))
    })?;

    let res = sqlx::query(
        "UPDATE dispatchers SET revoked_at = COALESCE(revoked_at, now()), deleted_at = now() \
         WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2) AND deleted_at IS NULL",
    )
    .bind(id)
    .bind(ctx.owner_filter())
    .execute(&state.pool)
    .await
    .map_err(|e| db_err(&e))?;

    if res.rows_affected() == 0 {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "dispatcher not found" })),
        ));
    }
    // Drop any live connection so the dispatcher can't keep operating under a
    // removed identity (it'll fail to re-auth on reconnect).
    state.dispatcher_connections.remove(&id);
    Ok(StatusCode::NO_CONTENT)
}
