//! Machine enrollment and connectivity shared with core, plus workspace telemetry.
use crate::context::DispatchCore;
use crate::targets::machine_status::{MachineBandwidth, MachineStatusResponse};
use axum::Json;
use axum::extract::{Path, State};
use cctui_server_kernel::error::AppError;
use uuid::Uuid;

pub async fn machine_status<S: DispatchCore>(
    State(state): State<S>,
    Path(machine_id): Path<Uuid>,
) -> Result<Json<MachineStatusResponse>, AppError> {
    let Json(mut status) =
        crate::targets::machine_status::machine_status(State(state.clone()), Path(machine_id))
            .await?;
    // Preserve peer-presence and bandwidth reporting in the full host.
    if !status.connected {
        status.connected = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM ws_presence WHERE kind = 'daemon' AND entity_id = $1 AND heartbeat_at > now() - interval '45 seconds')"
        ).bind(machine_id).fetch_one(state.pool()).await.unwrap_or(false);
    }
    status.bandwidth = sqlx::query_as::<_, MachineBandwidth>(
        "SELECT forward, retransmit, backfill, self_update, blob_put, heartbeat, updated_at FROM machine_bandwidth WHERE machine_id = $1"
    ).bind(machine_id).fetch_optional(state.pool()).await.unwrap_or(None);
    Ok(Json(status))
}
