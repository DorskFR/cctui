//! Execution-target connectivity snapshot.
use crate::error::AppError;
use crate::state::CoreState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

/// Connectivity snapshot used by daemon enrollment verification.
///
/// Hosts enforce read scope and resource access before serving this snapshot.
#[derive(Serialize)]
pub struct MachineStatusResponse {
    pub machine_id: Uuid,
    pub name: String,
    /// A daemon WS for this machine is currently connected. Stronger than `liveness`,
    /// which a just-inserted row satisfies before any daemon ever connects.
    pub connected: bool,
    pub liveness: cctui_proto::models::MachineLiveness,
    pub last_seen_at: DateTime<Utc>,
    pub revoked: bool,
    /// Last-known per-subsystem daemon bandwidth. `None` until the
    /// machine's daemon has sent a heartbeat carrying counters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bandwidth: Option<MachineBandwidth>,
}

/// Persisted per-subsystem byte totals for one machine.
#[derive(Serialize, sqlx::FromRow)]
pub struct MachineBandwidth {
    pub forward: i64,
    pub retransmit: i64,
    pub backfill: i64,
    pub self_update: i64,
    pub blob_put: i64,
    pub heartbeat: i64,
    pub updated_at: DateTime<Utc>,
}

pub async fn machine_status(
    State(state): State<CoreState>,
    Path(machine_id): Path<Uuid>,
) -> Result<Json<MachineStatusResponse>, AppError> {
    let row: Option<(String, DateTime<Utc>, Option<DateTime<Utc>>)> = sqlx::query_as(
        "SELECT name, last_seen_at, revoked_at FROM machines \
         WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(machine_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((name, last_seen_at, revoked_at)) = row else {
        return Err(AppError::new(StatusCode::NOT_FOUND, "machine not found"));
    };
    let connected = state.bus.daemon_connected(machine_id);
    Ok(Json(MachineStatusResponse {
        machine_id,
        name,
        connected,
        liveness: crate::targets::derive_liveness(last_seen_at),
        last_seen_at,
        revoked: revoked_at.is_some(),
        bandwidth: None,
    }))
}
