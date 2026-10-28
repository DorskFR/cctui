//! Machine enrollment and connectivity shared with core, plus workspace telemetry.
use crate::error::AppError;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
pub use cctui_server_core::targets::enrollment::{deenroll, enroll};
use cctui_server_core::targets::machine_status::{MachineBandwidth, MachineStatusResponse};
use uuid::Uuid;

pub async fn machine_status(
    State(state): State<AppState>,
    Path(machine_id): Path<Uuid>,
) -> Result<Json<MachineStatusResponse>, AppError> {
    let Json(mut status) = cctui_server_core::targets::machine_status::machine_status(
        State(state.core.clone()),
        Path(machine_id),
    )
    .await?;
    // Preserve peer-presence and bandwidth reporting in the full host.
    if !status.connected {
        status.connected = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM ws_presence WHERE kind = 'daemon' AND entity_id = $1 AND heartbeat_at > now() - interval '45 seconds')"
        ).bind(machine_id).fetch_one(&state.pool).await.unwrap_or(false);
    }
    status.bandwidth = sqlx::query_as::<_, MachineBandwidth>(
        "SELECT forward, retransmit, backfill, self_update, blob_put, heartbeat, updated_at FROM machine_bandwidth WHERE machine_id = $1"
    ).bind(machine_id).fetch_optional(&state.pool).await.unwrap_or(None);
    Ok(Json(status))
}
