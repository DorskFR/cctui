//! `POST /api/v1/sessions/spawn` (CCT-95).
//!
//! Pushes an `AdapterCommand::Spawn` to the targeted daemon over the
//! existing WS command channel. The daemon's adapter resolves the spawn
//! against its underlying agent (claude-code dispatches via the
//! `claude daemon` control socket; codex parity follows in CCT-98).
//!
//! Failure modes:
//!   * Daemon offline → 503 with hint.
//!   * Machine not owned by the requesting user → 403.
//!   * Unknown machine → 404.
//!
//! Mapping the returned `command_id` to the eventual `session_id` is the
//! client's job: it watches `/sessions` (or the TUI WS) for a new live
//! session and matches on `(machine_id, working_dir, registered_at >=
//! request_time)`. A future iteration can plumb the daemon's spawn ACK
//! back through the WS for an explicit mapping.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};

use cctui_proto::adapter::{AdapterCommand, AdapterId, SessionSpec};
use cctui_proto::api::{ApiError, SpawnRequest, SpawnResponse};
use cctui_proto::ws::DaemonFrameDown;
use uuid::Uuid;

use crate::auth::{AuthContext, TokenRole};
use crate::registry::MachineCommand;
use crate::state::AppState;

pub async fn spawn_session(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<SpawnRequest>,
) -> Result<(StatusCode, Json<SpawnResponse>), (StatusCode, Json<ApiError>)> {
    let machine_uuid = Uuid::parse_str(&req.machine_id).map_err(|_| {
        (StatusCode::BAD_REQUEST, Json(ApiError { error: "machine_id must be a uuid".into() }))
    })?;
    let row: Option<(Uuid,)> = sqlx::query_as("SELECT user_id FROM machines WHERE id = $1")
        .bind(machine_uuid)
        .fetch_optional(&state.pool)
        .await
        .map_err(|e| {
            tracing::error!("db error: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "database error".into() }))
        })?;
    let Some((owner,)) = row else {
        return Err((StatusCode::NOT_FOUND, Json(ApiError { error: "machine not found".into() })));
    };
    let permitted =
        matches!(ctx.role, TokenRole::Admin) || ctx.user_id.is_some_and(|uid| uid == owner);
    if !permitted {
        return Err((StatusCode::FORBIDDEN, Json(ApiError { error: "not your machine".into() })));
    }

    let adapter_id = req.adapter_id.clone().unwrap_or_else(|| "claude-code".to_owned());
    let spec = SessionSpec {
        adapter_id: AdapterId::new(&adapter_id),
        working_dir: Some(req.working_dir.clone()),
        prompt: req.prompt.clone(),
        name: req.name.clone(),
        permission_mode: req.permission_mode,
        bootstrap: serde_json::Value::Null,
    };
    // Mint the correlation id up front so it travels with the command and
    // comes back in an `AdapterEvent::CommandResult` → `ServerEvent::CommandResult`,
    // letting the client surface success/failure instead of silently polling.
    let command_id = Uuid::new_v4();
    let frame = DaemonFrameDown::Command {
        adapter_id: adapter_id.clone(),
        command: AdapterCommand::Spawn { spec, command_id: Some(command_id) },
    };

    let sender = state.daemon_connections.get(&machine_uuid).map(|r| r.clone());
    let Some(sender) = sender else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiError {
                error: "daemon for that machine is offline — start `cctui-daemon` first".into(),
            }),
        ));
    };
    if sender.send(frame).await.is_err() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiError { error: "daemon disconnected mid-dispatch".into() }),
        ));
    }

    tracing::info!(machine = %req.machine_id, %command_id, %adapter_id, "spawn dispatched");
    Ok((StatusCode::ACCEPTED, Json(SpawnResponse { command_id, status: "dispatched".into() })))
}

/// Legacy poll endpoint — superseded by WS push. Retained so older
/// clients that still poll get an empty list rather than a 404.
pub async fn get_machine_commands(
    State(state): State<AppState>,
    Path(machine_id): Path<String>,
) -> Json<Vec<MachineCommand>> {
    let commands = {
        let mut registry = state.registry.write().await;
        registry.take_machine_commands(&machine_id)
    };
    Json(commands)
}
