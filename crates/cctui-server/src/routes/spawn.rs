use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;

use crate::registry::MachineCommand;
use crate::state::AppState;
use cctui_proto::api::{SpawnRequest, SpawnResponse};

pub async fn spawn_session(
    State(state): State<AppState>,
    Json(req): Json<SpawnRequest>,
) -> (StatusCode, Json<SpawnResponse>) {
    let payload = serde_json::json!({
        "working_dir": req.working_dir,
        "prompt": req.prompt,
        "prompt_name": req.prompt_name,
    });
    let command_id = {
        let mut registry = state.registry.write().await;
        registry.queue_machine_command(&req.machine_id, "spawn", payload)
    };
    tracing::info!(machine = %req.machine_id, %command_id, "spawn command queued");
    (StatusCode::ACCEPTED, Json(SpawnResponse { command_id, status: "queued".into() }))
}

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
