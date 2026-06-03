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

use axum::extract::{Multipart, Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use base64::Engine;

use cctui_proto::adapter::{
    AdapterCommand, AdapterId, BootstrapFile, BootstrapUploads, SessionSpec,
};
use cctui_proto::api::{ApiError, SpawnRequest, SpawnResponse};
use cctui_proto::ws::DaemonFrameDown;
use uuid::Uuid;

use crate::auth::{AuthContext, TokenRole};
use crate::registry::MachineCommand;
use crate::state::AppState;

/// Upload caps (CCT-203). The bytes ride the server→daemon WS leg as base64
/// inside a single JSON frame, so this is deliberately a "attach a screenshot /
/// small doc" budget, not bulk transfer. The route's `DefaultBodyLimit` is set
/// above the total so an over-cap upload is rejected here with a clear 413
/// rather than a generic body-limit error.
const MAX_FILE_BYTES: usize = 5 * 1024 * 1024;
const MAX_TOTAL_BYTES: usize = 20 * 1024 * 1024;
const MAX_FILES: usize = 10;

fn bad_request(msg: impl Into<String>) -> (StatusCode, Json<ApiError>) {
    (StatusCode::BAD_REQUEST, Json(ApiError { error: msg.into() }))
}

/// Reduce an uploaded filename to a safe bare name: strip any directory
/// component and reject traversal (`..`)/empty/`.`. Prevents a malicious
/// `name` from escaping `/tmp/cctui-uploads/<session-id>/`.
fn sanitize_upload_name(raw: &str) -> Result<String, (StatusCode, Json<ApiError>)> {
    let base = std::path::Path::new(raw)
        .file_name()
        .and_then(|s| s.to_str())
        .map(str::to_owned)
        .ok_or_else(|| bad_request(format!("invalid upload filename: {raw:?}")))?;
    if base.is_empty() || base == ".." || base == "." {
        return Err(bad_request(format!("invalid upload filename: {raw:?}")));
    }
    Ok(base)
}

/// `POST /api/v1/sessions/spawn` — `multipart/form-data` (CCT-203).
///
/// Parts:
///   * `request` — the JSON [`SpawnRequest`] (machine, cwd, prompt, env, …).
///   * any part with a `filename` — a file to stage for the worker.
///
/// Files are base64-encoded into `SessionSpec.bootstrap` (the WS leg is JSON);
/// the daemon decodes + writes them to `/tmp/cctui-uploads/<session-id>/` and
/// references their paths in the prompt. `env` secrets ride on `SessionSpec.env`
/// (never persisted/logged) and the daemon injects them into the worker process.
#[allow(clippy::too_many_lines)]
pub async fn spawn_session(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    mut multipart: Multipart,
) -> Result<(StatusCode, Json<SpawnResponse>), (StatusCode, Json<ApiError>)> {
    let mut req: Option<SpawnRequest> = None;
    let mut uploads: Vec<BootstrapFile> = Vec::new();
    let mut total_bytes = 0usize;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| bad_request(format!("malformed multipart body: {e}")))?
    {
        let field_name = field.name().map(str::to_owned);
        let file_name = field.file_name().map(str::to_owned);
        if let Some(raw_name) = file_name {
            // A file part.
            let name = sanitize_upload_name(&raw_name)?;
            let bytes = field
                .bytes()
                .await
                .map_err(|e| bad_request(format!("reading upload {name:?}: {e}")))?;
            if bytes.len() > MAX_FILE_BYTES {
                return Err((
                    StatusCode::PAYLOAD_TOO_LARGE,
                    Json(ApiError {
                        error: format!(
                            "file {name:?} is {} bytes; per-file cap is {MAX_FILE_BYTES}",
                            bytes.len()
                        ),
                    }),
                ));
            }
            total_bytes += bytes.len();
            if total_bytes > MAX_TOTAL_BYTES {
                return Err((
                    StatusCode::PAYLOAD_TOO_LARGE,
                    Json(ApiError {
                        error: format!("uploads exceed the {MAX_TOTAL_BYTES}-byte total cap"),
                    }),
                ));
            }
            uploads.push(BootstrapFile {
                name,
                content_b64: base64::engine::general_purpose::STANDARD.encode(&bytes),
            });
            if uploads.len() > MAX_FILES {
                return Err((
                    StatusCode::PAYLOAD_TOO_LARGE,
                    Json(ApiError { error: format!("too many files; cap is {MAX_FILES}") }),
                ));
            }
        } else if field_name.as_deref() == Some("request") {
            let raw = field
                .text()
                .await
                .map_err(|e| bad_request(format!("reading request part: {e}")))?;
            req = Some(
                serde_json::from_str(&raw)
                    .map_err(|e| bad_request(format!("invalid SpawnRequest JSON: {e}")))?,
            );
        }
        // Unknown non-file parts are ignored.
    }

    let req = req.ok_or_else(|| bad_request("missing `request` part"))?;

    // Validate env keys (CCT-202): shell-style `^[A-Z_][A-Z0-9_]*$`.
    for key in req.env.keys() {
        let ok = !key.is_empty()
            && key.bytes().next().is_some_and(|b| b == b'_' || b.is_ascii_uppercase())
            && key.bytes().all(|b| b == b'_' || b.is_ascii_uppercase() || b.is_ascii_digit());
        if !ok {
            return Err(bad_request(format!("invalid env key {key:?} (want ^[A-Z_][A-Z0-9_]*$)")));
        }
    }

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
    let bootstrap = if uploads.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::to_value(BootstrapUploads { uploads }).map_err(|e| {
            tracing::error!("serializing bootstrap uploads: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError { error: "could not encode uploads".into() }),
            )
        })?
    };
    let spec = SessionSpec {
        adapter_id: AdapterId::new(&adapter_id),
        working_dir: Some(req.working_dir.clone()),
        prompt: req.prompt.clone(),
        name: req.name.clone(),
        permission_mode: req.permission_mode,
        effort: req.effort.clone().filter(|e| !e.trim().is_empty()),
        env: req.env.clone(),
        bootstrap,
    };
    // Mint the correlation id up front so it travels with the command and
    // comes back in an `AdapterEvent::CommandResult` → `ServerEvent::CommandResult`,
    // letting the client surface success/failure instead of silently polling.
    let command_id = Uuid::new_v4();
    let frame = DaemonFrameDown::Command {
        adapter_id: adapter_id.clone(),
        command: Box::new(AdapterCommand::Spawn { spec, command_id: Some(command_id) }),
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
