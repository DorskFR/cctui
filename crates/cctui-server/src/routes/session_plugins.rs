//! `PATCH /sessions/{id}/plugins/{plugin_id}` — the per-session plugin data
//! slot. Shape rules and the allow-list live in `crate::session_plugins`.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;

use crate::error::AppError;
use crate::session_plugins::{MAX_SLOT_BYTES, apply_slot, slot_allowed, slot_size};
use crate::state::AppState;

#[derive(Debug, serde::Deserialize)]
pub struct SlotRequest {
    /// Absent or `null` clears the slot.
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

pub async fn set_session_plugin(
    State(state): State<AppState>,
    Path((session_id, plugin_id)): Path<(String, String)>,
    Json(req): Json<SlotRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !slot_allowed(&plugin_id, state.plugins.get(&plugin_id).is_some()) {
        return Err(AppError::new(StatusCode::NOT_FOUND, "unknown plugin"));
    }
    if let Some(data) = &req.data {
        if slot_size(data) > MAX_SLOT_BYTES {
            return Err(AppError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                format!("plugin slot must be at most {MAX_SLOT_BYTES} bytes"),
            ));
        }
    }

    let current: Option<(serde_json::Value,)> =
        sqlx::query_as("SELECT metadata FROM sessions WHERE id = $1")
            .bind(&session_id)
            .fetch_optional(&state.pool)
            .await?;
    let Some((mut metadata,)) = current else {
        return Err(AppError::new(StatusCode::NOT_FOUND, "session not found"));
    };
    apply_slot(&mut metadata, &plugin_id, req.data.clone());

    sqlx::query("UPDATE sessions SET metadata = $2 WHERE id = $1")
        .bind(&session_id)
        .bind(&metadata)
        .execute(&state.pool)
        .await?;

    // A live session's list item is served from the registry, not the row, so
    // the in-memory copy has to move with the write or the chip lags a restart.
    {
        let mut registry = state.registry.write().await;
        if let Some(handle) = registry.get_mut(&session_id) {
            apply_slot(&mut handle.session.metadata, &plugin_id, req.data);
        }
    }

    tracing::info!(session = %session_id, plugin = %plugin_id, "plugin slot written");
    Ok(Json(metadata.get("plugins").cloned().unwrap_or_else(|| serde_json::json!({}))))
}
