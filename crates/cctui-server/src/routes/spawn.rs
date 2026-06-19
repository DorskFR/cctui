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

use cctui_proto::adapter::{AdapterCommand, AdapterId, BootstrapUploads, SessionSpec};
use cctui_proto::api::{ApiError, SpawnRequest, SpawnResponse};
use cctui_proto::ws::DaemonFrameDown;
use uuid::Uuid;

use crate::auth::AuthContext;
use crate::registry::MachineCommand;
use crate::state::AppState;
use crate::uploads::parse_upload_multipart;

fn bad_request(msg: impl Into<String>) -> (StatusCode, Json<ApiError>) {
    (StatusCode::BAD_REQUEST, Json(ApiError { error: msg.into() }))
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
    multipart: Multipart,
) -> Result<(StatusCode, Json<SpawnResponse>), (StatusCode, Json<ApiError>)> {
    let parsed = parse_upload_multipart(multipart).await?;
    let uploads = parsed.files;
    let req: SpawnRequest = parsed
        .request_json
        .ok_or_else(|| bad_request("missing `request` part"))
        .and_then(|raw| {
            serde_json::from_str(&raw)
                .map_err(|e| bad_request(format!("invalid SpawnRequest JSON: {e}")))
        })?;

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
    let permitted = ctx.is_admin() || ctx.user_id == owner;
    if !permitted {
        return Err((StatusCode::FORBIDDEN, Json(ApiError { error: "not your machine".into() })));
    }

    let adapter_id = req.adapter_id.clone().unwrap_or_else(|| "claude-code".to_owned());

    // OAuth account selection (CCT-232): if the caller picked a named account,
    // mint a session-scoped gateway token bound to it and inject the gateway
    // base-url + token into the worker env. Raw OAuth tokens never leave the
    // server. The session id isn't known until the worker registers, so the
    // command_id keys the (token → account) mapping for revocation.
    let command_id = Uuid::new_v4();
    let mut env = req.env.clone();
    // The session's model before any per-account remapping (CCT-406). When a
    // named account is selected below, its alias map can rewrite this to a
    // concrete model id (e.g. `opus` → `claude-opus-4-8[1m]`).
    let mut model = req.model.clone().filter(|m| !m.trim().is_empty());
    if let Some(account_name) = req.account.as_deref().filter(|a| !a.trim().is_empty()) {
        // Accounts are user-owned. The admin token has no user identity, so it
        // resolves the account against the target machine's owner (CCT-251) —
        // the session runs on that user's machine with that user's account.
        let uid = ctx.god_view_uid().unwrap_or(owner);
        // Resolve the model through this account's alias map (CCT-406) before it
        // reaches the worker — a no-op when the account has no matching alias.
        if let Some(m) = model.as_deref() {
            model = Some(
                crate::routes::gateway::resolve_account_model(
                    &state,
                    uid,
                    account_name,
                    req.provider.as_deref().filter(|p| !p.trim().is_empty()),
                    &adapter_id,
                    m,
                )
                .await,
            );
        }
        // The account drives the base URL + family (CCT-399); the explicit
        // `provider` disambiguates a name shared across providers, falling back
        // to the adapter-derived family when absent.
        match crate::routes::gateway::mint_session_env(
            &state,
            uid,
            account_name,
            req.provider.as_deref().filter(|p| !p.trim().is_empty()),
            &adapter_id,
            &command_id.to_string(),
        )
        .await
        {
            Ok(Some(gateway_env)) => env.extend(gateway_env),
            Ok(None) => {
                return Err((
                    StatusCode::NOT_FOUND,
                    Json(ApiError {
                        error: format!("no account named {account_name:?} for this adapter"),
                    }),
                ));
            }
            Err(e) => {
                tracing::error!("mint_session_env failed: {e}");
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiError { error: "could not provision account session".into() }),
                ));
            }
        }
    }

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
        model,
        env,
        bootstrap,
    };
    // `command_id` (minted above) travels with the command and comes back in an
    // `AdapterEvent::CommandResult` → `ServerEvent::CommandResult`, letting the
    // client surface success/failure instead of silently polling.
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

/// `POST /api/v1/sessions/{id}/files` — `multipart/form-data` (CCT-236).
///
/// Mid-chat file attachments. Same multipart shape + caps as `/sessions/spawn`
/// (one shared helper, [`crate::uploads::parse_upload_multipart`]); files are
/// forwarded to the owning daemon over the existing WS as a `StageFiles` op and
/// staged into the same per-session dir used at spawn time. Returns the staged
/// absolute paths so the client can reference them under the reply prompt.
pub async fn stage_session_files(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    multipart: Multipart,
) -> Result<Json<cctui_proto::api::StageFilesResponse>, (StatusCode, Json<ApiError>)> {
    let parsed = parse_upload_multipart(multipart).await?;
    if parsed.files.is_empty() {
        return Err(bad_request("no files in upload"));
    }
    let count = parsed.files.len();
    match crate::daemon_dispatch::stage_files(&state, &session_id, parsed.files).await {
        Ok(paths) => {
            tracing::info!(%session_id, count, "staged mid-chat files");
            Ok(Json(cctui_proto::api::StageFilesResponse { paths }))
        }
        Err(crate::daemon_dispatch::Error::NotFound) => {
            Err((StatusCode::NOT_FOUND, Json(ApiError { error: "session not found".into() })))
        }
        Err(
            err @ (crate::daemon_dispatch::Error::NoDaemon(_)
            | crate::daemon_dispatch::Error::Closed),
        ) => Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiError {
                error: format!("{err} — the session's machine is offline; try again"),
            }),
        )),
        Err(crate::daemon_dispatch::Error::Timeout) => Err((
            StatusCode::GATEWAY_TIMEOUT,
            Json(ApiError { error: "timed out staging files on the session's machine".into() }),
        )),
        Err(err @ crate::daemon_dispatch::Error::Staging(_)) => {
            Err((StatusCode::BAD_GATEWAY, Json(ApiError { error: err.to_string() })))
        }
        Err(err) => {
            tracing::error!(%session_id, %err, "stage_files dispatch error");
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError { error: "could not stage files".into() }),
            ))
        }
    }
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
