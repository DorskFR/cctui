//! Daemon ↔ Server contract surface.
//!
//! Three endpoints:
//!   * `POST /api/v1/daemon/auth` — daemon confirms identity, receives
//!     `machine_id` + `user_id` so it doesn't have to know them out of band.
//!   * `GET  /api/v1/daemon/ws`   — long-lived bidirectional WS. Daemon
//!     sends [`DaemonFrameUp`]; server sends [`DaemonFrameDown`]. On
//!     connect the server emits a [`DaemonFrameDown::Reconcile`] built
//!     from `adapters_enabled`.
//!   * `POST /api/v1/daemon/users/{id}/tokens` — mint a `user_tokens` row.
//!
//! Authentication: `machine_key` (Bearer) on every call; the regular
//! `auth_middleware` resolves it to `TokenRole::Machine` with
//! `machine_id` + `user_id` populated.

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path, State, WebSocketUpgrade};
use axum::http::{StatusCode, Uri};
use axum::response::IntoResponse;
use axum::{Extension, Json, response};
use cctui_proto::adapter::{AdapterEvent, AdapterId, EndReason};
use cctui_proto::api::{ApiError, DaemonAdapterConfig, DaemonAuthRequest, DaemonAuthResponse};
use cctui_proto::ws::{DaemonFrameDown, DaemonFrameUp};
use chrono::Utc;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::auth::{AuthContext, mint_secret, sha256_hex, user_token};
use crate::state::AppState;

/// Evict a daemon whose WS produced no frame within this window. A healthy
/// daemon pings every ~20s (auto-ponged), so 3× that distinguishes a
/// half-open connection from idleness (CCT-140).
const DAEMON_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

// ---- /api/v1/daemon/auth ----

/// Daemon presents its long-lived machine key (or a session token re-issued
/// from one). Returns the machine + owning user so the daemon can label
/// itself and avoid out-of-band configuration. v0 returns the same machine
/// key back as the session token; post-v0 may issue a short-lived JWT.
pub async fn auth(
    State(state): State<AppState>,
    Json(req): Json<DaemonAuthRequest>,
) -> Result<Json<DaemonAuthResponse>, (StatusCode, Json<ApiError>)> {
    let ctx = state.auth_config.validate(&req.machine_key).await.ok_or_else(|| {
        (StatusCode::UNAUTHORIZED, Json(ApiError { error: "invalid machine key".into() }))
    })?;
    let Some(machine_id) = ctx.machine_id else {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ApiError { error: "machine token required".into() }),
        ));
    };
    Ok(Json(DaemonAuthResponse {
        session_token: req.machine_key,
        expires_at: Utc::now() + chrono::Duration::hours(24),
        machine_id,
        user_id: ctx.user_id,
    }))
}

// ---- /api/v1/daemon/sessions/{id}/gateway-env ----

/// Resolve a session's gateway-routing env for the daemon's launch chokepoint
/// (CCT-460). The daemon calls this at every worker (re)launch — spawn, resume,
/// cold-resume, fork — so the gateway credential comes from the server's durable
/// `sessions.account_id` binding rather than from whatever env the triggering
/// command happened to carry. This is what makes routing survive a daemon /
/// claude-daemon restart and session-id rotation: the env is re-derived from the
/// DB, not from volatile process/in-memory state.
///
/// Self-authenticating like [`auth`]/[`ws`]: the machine key is the Bearer.
/// Scoped to the machine's owning user so a daemon can't resolve another user's
/// account env.
///
/// Returns `{account_bound, env}`:
///   * no binding → `{false, {}}` (no gateway routing needed; launch as-is)
///   * bound + mintable → `{true, env}` (inject and launch)
///   * bound + unmintable (account gone) → `{true, {}}` (daemon fails closed)
///   * transient DB error → 500 (daemon falls back to the pushed env hint)
pub async fn session_gateway_env(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(session_id): Path<String>,
) -> Result<Json<cctui_proto::api::GatewayEnvResponse>, StatusCode> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let ctx = state.auth_config.validate(token).await.ok_or(StatusCode::UNAUTHORIZED)?;
    if ctx.machine_id.is_none() {
        return Err(StatusCode::FORBIDDEN);
    }

    // User-scope: only resolve env for sessions owned by the machine's user. A
    // session row that exists but belongs to another user yields "not bound"
    // (the daemon launches without gateway env) rather than leaking that user's
    // account credential. A missing row (spawn-time race before register) is
    // allowed through — the account resolves via the freshly-minted token row.
    let owner: Option<Uuid> = sqlx::query_scalar("SELECT user_id FROM sessions WHERE id = $1")
        .bind(&session_id)
        .fetch_optional(&state.pool)
        .await
        .ok()
        .flatten();
    if owner.is_some_and(|o| o != ctx.user_id) {
        return Ok(Json(cctui_proto::api::GatewayEnvResponse {
            account_bound: false,
            env: std::collections::BTreeMap::default(),
            settings: None,
        }));
    }

    // Resolve EVERY bound family (one account per family) and re-mint each, so a
    // worker carrying both claude + codex creds gets both restored on launch,
    // not just the last-minted family (CCT-514). The families emit disjoint env
    // keys, so the merge never collides.
    let accounts = crate::routes::gateway::resolve_session_accounts(&state, &session_id).await;
    if accounts.is_empty() {
        return Ok(Json(cctui_proto::api::GatewayEnvResponse {
            account_bound: false,
            env: std::collections::BTreeMap::default(),
            settings: None,
        }));
    }
    let mut env = std::collections::BTreeMap::new();
    for account_id in accounts {
        match crate::routes::gateway::mint_session_env_for_account(&state, account_id, &session_id)
            .await
        {
            Ok(Some(e)) => env.extend(e),
            // This family's account row is gone — skip it; other families may
            // still mint. With every family gone, `env` stays empty and we report
            // bound + empty below so the daemon fails closed instead of launching
            // a worker that will 401.
            Ok(None) => {}
            // Transient DB failure: let the daemon fall back to its pushed env hint.
            Err(e) => {
                tracing::error!(%session_id, "daemon gateway-env mint failed: {e}");
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            }
        }
    }
    // Merge the bound account(s)' per-account `settings_json` (CCT-539) so it
    // rides alongside the gateway env on this same pull. Re-served on every
    // (re)launch, it survives a daemon / claude-daemon restart; the daemon
    // deep-merges it UNDER its managed hook settings when writing the worker's
    // `--settings` file (that daemon-side merge is CCT-540).
    let settings = crate::routes::gateway::resolve_session_settings(&state, &session_id).await;
    // This pull only happens when the daemon is actually (re)launching the
    // worker — a session marked `ended` (possibly by a spurious end, CCT-565)
    // is provably coming back to life, so un-stick the terminal status here.
    // `archived` stays parked: un-archiving is an explicit user action.
    let _ = sqlx::query("UPDATE sessions SET status = 'active' WHERE id = $1 AND status = 'ended'")
        .bind(&session_id)
        .execute(&state.pool)
        .await;
    Ok(Json(cctui_proto::api::GatewayEnvResponse { account_bound: true, env, settings }))
}

// ---- /api/v1/daemon/sessions/{id}/token-valid ----

/// Query for [`session_token_valid`]: the sha256 hex of the session token the
/// daemon launched the worker with. Hash-only on purpose — no token material
/// on the wire (CCT-503 invariant).
#[derive(Deserialize)]
pub struct TokenValidQuery {
    pub hash: String,
}

/// Does this session's minted token still resolve at the gateway? (CCT-462)
///
/// The daemon's validity sweep calls this for TRUSTED workers (ones it
/// launched with gateway env) so a worker whose `session_tokens` row got
/// unbound/deleted — which 401s forever at the gateway session-token stage —
/// is finally observable and healable, instead of relying purely on
/// launch-trust memory. `valid` = a `session_tokens` row with this hash exists
/// FOR THIS SESSION, is not revoked, and joins a live `account_providers` row
/// (the same join [`resolve_account`](crate::routes::gateway) applies, but by
/// hash equality).
///
/// Self-authenticating like [`session_gateway_env`]: machine-key Bearer.
/// User-scoped the same way, except a session owned by another user answers
/// 404 rather than `{valid: false}` — a false `valid` triggers a destructive
/// kill + cold-resume daemon-side, and the daemon treats any non-200 as
/// "unknown" (no heal). Transient DB error → 500 for the same reason
/// (fail-open; the heal kill is destructive).
pub async fn session_token_valid(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(session_id): Path<String>,
    axum::extract::Query(q): axum::extract::Query<TokenValidQuery>,
) -> Result<Json<cctui_proto::api::TokenValidResponse>, StatusCode> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let ctx = state.auth_config.validate(token).await.ok_or(StatusCode::UNAUTHORIZED)?;
    if ctx.machine_id.is_none() {
        return Err(StatusCode::FORBIDDEN);
    }

    // User-scope (mirrors `session_gateway_env`): only answer for sessions
    // owned by the machine's user. A foreign session 404s — NOT `valid:false`,
    // which would trigger a destructive kill + cold-resume daemon-side.
    let owner: Option<Uuid> = sqlx::query_scalar("SELECT user_id FROM sessions WHERE id = $1")
        .bind(&session_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(|e| {
            tracing::error!(%session_id, "token-valid owner lookup failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    if owner.is_some_and(|o| o != ctx.user_id) {
        return Err(StatusCode::NOT_FOUND);
    }

    // Valid = a live token row with this hash, scoped to this session, that
    // still joins a live account (same join semantics as the gateway's
    // `resolve_account`, but by hash equality — the daemon sends the sha256
    // hex of the token it launched the worker with, never the token itself).
    let valid: bool = sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM session_tokens t JOIN account_providers a ON a.id = t.account_id
            WHERE t.token_hash = $1 AND t.session_id = $2 AND t.revoked_at IS NULL
         )",
    )
    .bind(&q.hash)
    .bind(&session_id)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!(%session_id, "token-valid lookup failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(Json(cctui_proto::api::TokenValidResponse { valid }))
}

// ---- /api/v1/daemon/ws ----

pub async fn ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    uri: Uri,
) -> Result<response::Response, StatusCode> {
    // WS upgrade can't carry an Authorization header from browsers, so we
    // accept `?token=` on the query string. CLI daemons can use either.
    let token = extract_token_from_uri(&uri).ok_or(StatusCode::UNAUTHORIZED)?;
    let ctx = state.auth_config.validate(&token).await.ok_or(StatusCode::UNAUTHORIZED)?;
    let Some(machine_id) = ctx.machine_id else {
        return Err(StatusCode::FORBIDDEN);
    };
    let user_id = ctx.user_id;
    Ok(ws.on_upgrade(move |socket| handle(socket, state, machine_id, user_id)).into_response())
}

fn extract_token_from_uri(uri: &Uri) -> Option<String> {
    uri.query().and_then(|q| {
        q.split('&').find_map(|param| {
            let mut parts = param.split('=');
            match (parts.next(), parts.next()) {
                (Some("token"), Some(token)) => Some(token.to_string()),
                _ => None,
            }
        })
    })
}

#[allow(clippy::cognitive_complexity)]
async fn handle(socket: WebSocket, state: AppState, machine_id: Uuid, user_id: Uuid) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::channel::<DaemonFrameDown>(64);

    // Register the daemon for command fan-out. If a stale entry exists,
    // overwrite it (newest connection wins).
    state.daemon_connections.insert(machine_id, tx.clone());

    // Send Reconcile immediately.
    match load_reconcile(&state, machine_id).await {
        Ok(adapters) => {
            if tx.send(DaemonFrameDown::Reconcile { adapters }).await.is_err() {
                tracing::warn!("daemon tx closed before reconcile");
            }
        }
        Err(err) => {
            tracing::error!(%err, "load_reconcile failed");
        }
    }

    // Outbound pump. Besides forwarding `DaemonFrameDown` frames, it sends a
    // periodic WS Ping so the daemon always hears from us within its liveness
    // window. Without this, an idle connection (no commands queued) sends the
    // daemon nothing after the initial Reconcile — axum does not auto-flush a
    // Pong on the split sink while it's otherwise idle — so the daemon's
    // half-open detector tears the WS down every 60s and flaps forever
    // (CCT-144). The interval mirrors the daemon's 20s ping cadence and stays
    // well under both sides' 60s timeouts.
    let outbound = tokio::spawn(async move {
        let mut keepalive = tokio::time::interval(std::time::Duration::from_secs(20));
        keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        keepalive.tick().await; // discard the immediate first tick
        loop {
            tokio::select! {
                frame = rx.recv() => {
                    let Some(frame) = frame else { break };
                    let Ok(json) = serde_json::to_string(&frame) else { continue };
                    if sink.send(Message::Text(json.into())).await.is_err() {
                        break;
                    }
                }
                _ = keepalive.tick() => {
                    if sink.send(Message::Ping(Vec::new().into())).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    // Inbound loop. A live daemon pings every ~20s, so a read timeout of 3×
    // that evicts a half-open daemon promptly instead of leaving a dead entry
    // in `daemon_connections` that swallows commands (CCT-140).
    loop {
        let msg = match tokio::time::timeout(DAEMON_READ_TIMEOUT, stream.next()).await {
            Ok(Some(Ok(msg))) => msg,
            Ok(Some(Err(_)) | None) => break,
            Err(_) => {
                tracing::warn!(%machine_id, "daemon WS idle past read timeout — evicting");
                break;
            }
        };
        let payload = match msg {
            Message::Text(t) => t.to_string(),
            Message::Binary(b) => String::from_utf8_lossy(&b).to_string(),
            Message::Close(_) => break,
            _ => continue,
        };
        let frame: DaemonFrameUp = match serde_json::from_str(&payload) {
            Ok(f) => f,
            Err(err) => {
                tracing::warn!(%err, "bad daemon frame");
                continue;
            }
        };
        let trace = frame_trace(&frame);
        if let Err(err) = process_frame(&state, machine_id, user_id, frame).await {
            tracing::warn!(%err, %trace, "process_frame error");
        }
    }

    // Cleanup. Only drop the entry if it is STILL OURS. During a reconnect
    // race the daemon's new connection may have already overwritten the map
    // with its own `tx` (line ~110, "newest wins"); an unconditional remove
    // here would delete that live channel, so every command would silently
    // fail `NoDaemon` while events kept flowing (they go through
    // `process_frame`, which never touches `daemon_connections`). Compare the
    // stored sender against ours and only remove a match (CCT-159).
    state.daemon_connections.remove_if(&machine_id, |_, current| current.same_channel(&tx));
    outbound.abort();
}

fn frame_trace(frame: &DaemonFrameUp) -> String {
    match frame {
        DaemonFrameUp::SessionRegistered { adapter_id, local_id } => {
            format!("session_registered adapter={adapter_id} local_id={local_id}")
        }
        DaemonFrameUp::Event { adapter_id, event } => {
            format!(
                "event adapter={adapter_id} kind={} local_id={}",
                event_kind(event),
                event_local_id(event),
            )
        }
        DaemonFrameUp::Heartbeat { .. } => "heartbeat".to_owned(),
        _ => "other".to_owned(),
    }
}

fn event_local_id(event: &AdapterEvent) -> &str {
    match event {
        AdapterEvent::SessionStarted { local_id, .. }
        | AdapterEvent::Message { local_id, .. }
        | AdapterEvent::ToolUse { local_id, .. }
        | AdapterEvent::SessionEnded { local_id, .. }
        | AdapterEvent::Status { local_id, .. }
        | AdapterEvent::PermissionRequest { local_id, .. }
        | AdapterEvent::PermissionResolved { local_id, .. }
        | AdapterEvent::TokenUsage { local_id, .. } => local_id,
        _ => "",
    }
}

const fn event_kind(event: &AdapterEvent) -> &'static str {
    match event {
        AdapterEvent::SessionStarted { .. } => "session_started",
        AdapterEvent::Message { .. } => "message",
        AdapterEvent::ToolUse { .. } => "tool_use",
        AdapterEvent::SessionEnded { .. } => "session_ended",
        AdapterEvent::Status { .. } => "status",
        AdapterEvent::TokenUsage { .. } => "token_usage",
        AdapterEvent::PermissionRequest { .. } => "permission_request",
        AdapterEvent::PermissionResolved { .. } => "permission_resolved",
        _ => "other",
    }
}

// Breadth-of-match dispatch over inbound daemon frames; complexity is per-frame
// handling, not nesting.
#[allow(clippy::cognitive_complexity)]
async fn process_frame(
    state: &AppState,
    machine_id: Uuid,
    user_id: Uuid,
    frame: DaemonFrameUp,
) -> anyhow::Result<()> {
    match frame {
        DaemonFrameUp::SessionRegistered { adapter_id, local_id } => {
            upsert_session(state, machine_id, user_id, &adapter_id, &local_id, None, None).await
        }
        DaemonFrameUp::Event { adapter_id, event } => {
            tracing::debug!(
                %adapter_id,
                kind = event_kind(&event),
                local_id = event_local_id(&event),
                "received event",
            );
            handle_event(state, machine_id, user_id, &adapter_id, event).await
        }
        DaemonFrameUp::StageFilesResult { request_id, ok, paths, error } => {
            // Mid-chat attachment reply (CCT-236): fire the oneshot the
            // `POST /sessions/{id}/files` route is awaiting.
            if let Some((_, reply_tx)) = state.pending_stage_requests.remove(&request_id) {
                let outcome = if ok {
                    Ok(paths)
                } else {
                    Err(error.unwrap_or_else(|| "daemon reported staging failure".to_owned()))
                };
                // Receiver gone (route timed out already) → drop silently.
                let _ = reply_tx.send(outcome);
            } else {
                tracing::debug!(%request_id, "StageFilesResult for unknown request (timed out?)");
            }
            Ok(())
        }
        DaemonFrameUp::ListDirsResult { request_id, ok, dirs, error } => {
            // Working-dir autocomplete reply: fire the oneshot the
            // `GET /machines/{id}/fs/dirs` route is awaiting.
            if let Some((_, reply_tx)) = state.pending_listdirs_requests.remove(&request_id) {
                let outcome = if ok {
                    Ok(dirs)
                } else {
                    Err(error.unwrap_or_else(|| "daemon reported a listing failure".to_owned()))
                };
                // Receiver gone (route timed out already) → drop silently.
                let _ = reply_tx.send(outcome);
            } else {
                tracing::debug!(%request_id, "ListDirsResult for unknown request (timed out?)");
            }
            Ok(())
        }
        DaemonFrameUp::Heartbeat { .. } => {
            // Machine liveness (CCT-255): advance `last_seen_at` on EVERY
            // heartbeat (not just connect, as auth.rs does), then derive the
            // online/stale/offline tier and broadcast it on transition. This is
            // the proactive signal the server previously lacked — a daemon that
            // stops heartbeating ages to offline without a failed dispatch.
            if let Err(err) = sqlx::query("UPDATE machines SET last_seen_at = now() WHERE id = $1")
                .bind(machine_id)
                .execute(&state.pool)
                .await
            {
                tracing::warn!(%err, %machine_id, "heartbeat last_seen_at bump failed");
            }
            crate::machine_liveness::record_and_broadcast(
                state,
                machine_id,
                cctui_proto::models::MachineLiveness::Online,
            );
            Ok(())
        }
        // Any future #[non_exhaustive] variants are no-ops.
        _ => Ok(()),
    }
}

#[allow(clippy::too_many_lines)]
#[allow(clippy::cognitive_complexity)]
async fn handle_event(
    state: &AppState,
    machine_id: Uuid,
    user_id: Uuid,
    adapter_id: &str,
    event: AdapterEvent,
) -> anyhow::Result<()> {
    let local_id_for_bump = match &event {
        AdapterEvent::Message { local_id, .. }
        | AdapterEvent::ToolUse { local_id, .. }
        | AdapterEvent::Status { local_id, .. } => Some(local_id.clone()),
        _ => None,
    };
    let broadcast_pair: Option<(String, cctui_proto::ws::AgentEvent)> = match &event {
        AdapterEvent::Message { local_id, payload } => {
            crate::normalize::to_agent_event(adapter_id, "message", payload)
                .map(|ae| (local_id.clone(), ae))
        }
        AdapterEvent::ToolUse { local_id, payload } => {
            crate::normalize::to_agent_event(adapter_id, "tool_use", payload)
                .map(|ae| (local_id.clone(), ae))
        }
        _ => None,
    };
    // Whether the broadcast below should actually fire. For Message/ToolUse we
    // only stream to the webui if the event was *newly* inserted — a daemon
    // that replays a session's full history on reconnect (e.g. after a
    // self-update) would otherwise re-stream every message, forcing clients to
    // replay the whole conversation with a long visible lag (CCT-171). The
    // `ON CONFLICT DO NOTHING` dedup already drops the duplicate rows; gating
    // the broadcast on a real insert extends that dedup to the live stream.
    let mut newly_inserted = true;
    match event {
        AdapterEvent::SessionStarted { local_id, meta } => {
            let working_dir = meta.working_dir.clone();
            upsert_session(
                state,
                machine_id,
                user_id,
                adapter_id,
                &local_id,
                working_dir,
                meta.parent_local_id.clone(),
            )
            .await?;
        }
        AdapterEvent::Message { local_id, payload } => {
            newly_inserted = insert_event(state, &local_id, "message", payload).await?;
        }
        AdapterEvent::ToolUse { local_id, payload } => {
            newly_inserted = insert_event(state, &local_id, "tool_use", payload).await?;
        }
        AdapterEvent::SessionEnded { local_id, reason } => {
            mark_session_ended(state, &local_id, &reason).await?;
        }
        AdapterEvent::TokenUsage {
            local_id,
            message_id,
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_creation_tokens,
        } => {
            insert_token_usage(
                state,
                &local_id,
                &message_id,
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_creation_tokens,
            )
            .await?;
        }
        AdapterEvent::CommandResult { command_id, ok, error } => {
            // Not a session event — rebroadcast straight to clients so the
            // originating spawn request gets a definitive answer (CCT-131).
            if !ok {
                tracing::warn!(%command_id, ?error, "command failed on daemon");
            }
            let _ = state.tui_tx.send(cctui_proto::ws::ServerEvent::CommandResult {
                command_id: command_id.to_string(),
                ok,
                error,
            });
        }
        AdapterEvent::PermissionRequest { local_id, request_id, tool, input } => {
            // `local_id` is the session id (claude session id / codex rollout
            // id, both used as the sessions PK). Park the request for TUI/web
            // and broadcast so inline prompts appear live.
            let input_preview = {
                let s = if input.is_null() { String::new() } else { input.to_string() };
                s.chars().take(500).collect::<String>()
            };
            // Auto-approve (CCT-151): if the session is in auto-approve mode,
            // answer `allow` immediately without prompting any client.
            if state.permission_store.read().await.is_auto_approve(&local_id) {
                tracing::info!(
                    session_id = %local_id,
                    request_id = %request_id,
                    tool = %tool,
                    "auto-approving permission request"
                );
                let _ = crate::daemon_dispatch::dispatch(
                    state,
                    &local_id,
                    cctui_proto::adapter::AdapterCommand::PermissionResponse {
                        local_id: local_id.clone(),
                        request_id: request_id.clone(),
                        allow: true,
                    },
                )
                .await;
                bump_heartbeat(state, &local_id).await;
                return Ok(());
            }
            state.permission_store.write().await.insert_request(
                crate::routes::permissions::PendingPermission {
                    session_id: local_id.clone(),
                    request_id: request_id.clone(),
                    tool_name: tool.clone(),
                    description: tool.clone(),
                    input_preview: input_preview.clone(),
                    received_at: chrono::Utc::now(),
                },
            );
            let _ = state.tui_tx.send(cctui_proto::ws::ServerEvent::PermissionRequest {
                session_id: local_id.clone(),
                request_id,
                tool_name: tool.clone(),
                description: tool,
                input_preview,
            });
            bump_heartbeat(state, &local_id).await;
        }
        AdapterEvent::PermissionResolved { local_id, request_id } => {
            // The adapter observed the agent's permission prompt clear (answered
            // natively, dispatched by us, or timed out). Drop the parked request
            // and tell clients to dismiss the inline prompt. Idempotent: a
            // request answered via cctui already broadcast PermissionResolved on
            // the client path, so a second clear here is a harmless no-op.
            state.permission_store.write().await.record_decision(&request_id, "resolved".into());
            let _ = state.tui_tx.send(cctui_proto::ws::ServerEvent::PermissionResolved {
                session_id: local_id.clone(),
                request_id,
            });
            bump_heartbeat(state, &local_id).await;
        }
        AdapterEvent::AskQuestion { local_id, question, questions, preamble } => {
            // Live AskUserQuestion (CCT-164): broadcast the pending question so
            // clients render an inline prompt immediately. Ephemeral — not
            // persisted as a stream_event; the full structured tool call still
            // lands in history via the transcript once the turn advances.
            // `questions` carries the structured options so the client renders
            // the interactive form live, not just the flattened text (CCT-181).
            // Park it authoritatively so a client that (re)subscribes after the
            // broadcast still learns the open prompt — the broadcast alone was
            // lost forever if nobody was listening at that instant (CCT-277).
            state.permission_store.write().await.insert_ask(
                crate::routes::permissions::PendingAsk {
                    session_id: local_id.clone(),
                    question: question.clone(),
                    questions: questions.clone(),
                    preamble: preamble.clone(),
                    received_at: chrono::Utc::now(),
                },
            );
            let _ = state.tui_tx.send(cctui_proto::ws::ServerEvent::AskQuestion {
                session_id: local_id.clone(),
                question,
                questions,
                preamble,
            });
            bump_heartbeat(state, &local_id).await;
        }
        AdapterEvent::AskResolved { local_id } => {
            state.permission_store.write().await.remove_ask(&local_id);
            let _ = state
                .tui_tx
                .send(cctui_proto::ws::ServerEvent::AskResolved { session_id: local_id.clone() });
            bump_heartbeat(state, &local_id).await;
        }
        AdapterEvent::PlanRequest { local_id, plan, preamble } => {
            // Live ExitPlanMode plan-approval prompt (CCT-347): park it
            // authoritatively (so a (re)subscribing client still learns it) and
            // broadcast so clients render the live Plan card. Mirrors the
            // AskQuestion path; ephemeral, not persisted as a stream_event.
            state.permission_store.write().await.insert_plan(
                crate::routes::permissions::PendingPlan {
                    session_id: local_id.clone(),
                    plan: plan.clone(),
                    preamble: preamble.clone(),
                    received_at: chrono::Utc::now(),
                },
            );
            let _ = state.tui_tx.send(cctui_proto::ws::ServerEvent::PlanRequest {
                session_id: local_id.clone(),
                plan,
                preamble,
            });
            bump_heartbeat(state, &local_id).await;
        }
        AdapterEvent::PlanResolved { local_id } => {
            state.permission_store.write().await.remove_plan(&local_id);
            let _ = state
                .tui_tx
                .send(cctui_proto::ws::ServerEvent::PlanResolved { session_id: local_id.clone() });
            bump_heartbeat(state, &local_id).await;
        }
        AdapterEvent::Status {
            local_id,
            tempo,
            state: agent_state,
            activity,
            name,
            model,
            effort,
            ..
        } => {
            // Persist the classifier signals + display metadata so
            // `list_sessions` can derive the "needs input" attention flag and
            // show name/model/effort. Status events are otherwise not stored
            // as stream_events (heartbeat bump below handles liveness).
            update_status_signals(
                state,
                &local_id,
                StatusSignals {
                    tempo: tempo.as_deref(),
                    agent_state: agent_state.as_deref(),
                    activity: activity.as_deref(),
                    name: name.as_deref(),
                    model: model.as_deref(),
                    effort: effort.as_deref(),
                },
            )
            .await?;
        }
        AdapterEvent::SessionModel { local_id, model } => {
            // Fill the model from the transcript's ground truth, but only when
            // it's still unset — an explicit `--model` alias delivered via a
            // Status event keeps priority (the alias the operator typed, e.g.
            // "opus", rather than the full id). Cheap indexed no-op once set.
            sqlx::query("UPDATE sessions SET model = $2 WHERE id = $1 AND model IS NULL")
                .bind(&local_id)
                .bind(&model)
                .execute(&state.pool)
                .await
                .map_err(|e| {
                    tracing::error!("db error (session model): {e}");
                    e
                })?;
        }
        _ => {}
    }
    if let Some(id) = local_id_for_bump {
        bump_heartbeat(state, &id).await;
    }
    if newly_inserted && let Some((session_id, data)) = broadcast_pair {
        let _ = state.tui_tx.send(cctui_proto::ws::ServerEvent::Stream { session_id, data });
    }
    Ok(())
}

/// Latest classifier signals + display metadata from a Status event.
struct StatusSignals<'a> {
    tempo: Option<&'a str>,
    agent_state: Option<&'a str>,
    activity: Option<&'a str>,
    name: Option<&'a str>,
    model: Option<&'a str>,
    effort: Option<&'a str>,
}

/// Persist the latest Status signals onto the session row. `COALESCE` keeps
/// a previously-known value when a given Status event omits a field, so a
/// sparse update never clears signal.
async fn update_status_signals(
    state: &AppState,
    local_id: &str,
    s: StatusSignals<'_>,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE sessions SET \
            tempo = COALESCE($2, tempo), \
            agent_state = COALESCE($3, agent_state), \
            activity = COALESCE($4, activity), \
            session_name = COALESCE($5, session_name), \
            model = COALESCE($6, model), \
            effort = COALESCE($7, effort) \
         WHERE id = $1",
    )
    .bind(local_id)
    .bind(s.tempo)
    .bind(s.agent_state)
    .bind(s.activity)
    .bind(s.name)
    .bind(s.model)
    .bind(s.effort)
    .execute(&state.pool)
    .await?;
    Ok(())
}

/// Bump `last_heartbeat` for a session and, when it is a subagent, the whole
/// `parent_id` chain up to the root (CCT-366). A subagent's work should keep
/// its parent(s) "alive" so the parent card doesn't read idle/stale (CCT-365)
/// while a child churns. Done as a single recursive CTE UPDATE — one round-trip
/// regardless of nesting depth, since subagents are chatty. Heartbeat only: no
/// token/usage aggregates touched, so there's no double-counting.
async fn bump_heartbeat(state: &AppState, local_id: &str) {
    if let Err(err) = sqlx::query(
        r"WITH RECURSIVE chain AS (
            SELECT id, parent_id FROM sessions WHERE id = $1
            UNION ALL
            SELECT s.id, s.parent_id FROM sessions s JOIN chain c ON s.id = c.parent_id
        )
        UPDATE sessions SET last_heartbeat = now()
        WHERE id IN (SELECT id FROM chain)",
    )
    .bind(local_id)
    .execute(&state.pool)
    .await
    {
        tracing::warn!(%err, %local_id, "heartbeat bump failed");
    }
}

async fn upsert_session(
    state: &AppState,
    machine_id: Uuid,
    user_id: Uuid,
    adapter_id: &str,
    local_id: &str,
    working_dir: Option<String>,
    parent_local_id: Option<String>,
) -> anyhow::Result<()> {
    // `parent_id` (CCT-141): resolve via a subquery rather than binding the
    // raw value so a not-yet-known parent yields NULL instead of an FK
    // violation that would drop the whole insert. In the normal case the
    // parent is upserted earlier in the same poll, so it resolves. On
    // conflict, COALESCE keeps an already-set parent and otherwise fills it
    // in from a later poll.
    //
    // `status` on conflict (CCT-275): the codex thread/list inventory poll
    // (CCT-263) re-emits SessionStarted for every machine-wide thread every
    // ~15s, including ones the user archived. Preserve terminal/parked states
    // (`inactive`, `archived`, `ended`) so a re-discovery refreshes the
    // heartbeat without resurrecting the session into the Working list;
    // otherwise revive it to `active`.
    sqlx::query(
        r"INSERT INTO sessions
            (id, parent_id, account_id, machine_id, working_dir, status, registered_at,
             last_heartbeat, metadata, user_id, machine_uuid, adapter_id)
          VALUES ($1, (SELECT id FROM sessions WHERE id = $7), NULL, $2, $3, 'active',
                  now(), now(), '{}'::jsonb, $4, $5, $6)
          ON CONFLICT (id) DO UPDATE SET
            last_heartbeat = now(),
            status = CASE WHEN sessions.status IN ('inactive', 'archived', 'ended') THEN sessions.status ELSE 'active' END,
            adapter_id = EXCLUDED.adapter_id,
            parent_id = COALESCE(sessions.parent_id, EXCLUDED.parent_id)",
    )
    .bind(local_id)
    .bind(machine_id.to_string())
    .bind(working_dir.unwrap_or_default())
    .bind(user_id)
    .bind(machine_id)
    .bind(adapter_id)
    .bind(parent_local_id)
    .execute(&state.pool)
    .await?;
    // Repair the durable account binding (CCT-565): the dispatch path mints the
    // gateway token BEFORE the daemon registers the session, so mint-time's
    // best-effort `UPDATE sessions SET account_id` hit no row and the binding
    // (CCT-460) silently stayed NULL — leaving resume-after-revocation with
    // nothing to re-mint from. Backfill it here from the newest token row
    // (live preferred). No-op for already-bound or never-bound sessions.
    sqlx::query(
        "UPDATE sessions SET account_id = ( \
            SELECT st.account_id::text FROM session_tokens st \
             WHERE st.session_id = $1 \
             ORDER BY (st.revoked_at IS NULL) DESC, st.created_at DESC LIMIT 1) \
         WHERE id = $1 AND account_id IS NULL",
    )
    .bind(local_id)
    .execute(&state.pool)
    .await?;
    Ok(())
}

/// Insert a stream event, returning `true` if a new row was written and
/// `false` if it was a duplicate suppressed by the dedup constraint. Callers
/// use the return value to decide whether to broadcast the event live, so a
/// replayed session history doesn't re-stream to clients (CCT-171).
async fn insert_event(
    state: &AppState,
    local_id: &str,
    event_type: &str,
    mut payload: serde_json::Value,
) -> anyhow::Result<bool> {
    // Postgres jsonb/text cannot store the NUL code point (`\0`); a
    // payload carrying one (e.g. binary-ish tool output) fails the INSERT and
    // the event is silently lost (CCT-136). Strip NULs from every string so
    // the rest of the payload survives.
    strip_nul(&mut payload);
    // Guard the insert on the session row existing. A daemon can emit an event
    // for a session the server never registered (e.g. a session_ended whose
    // prior SessionStarted was never received, or an ephemeral subagent
    // session): a bare INSERT then trips `stream_events_session_id_fkey`,
    // spamming WARN logs and burning a failed DB round-trip per event. The
    // `WHERE EXISTS` makes that case a clean no-op (0 rows) instead — when the
    // session is present this is identical to the old insert (CCT-493).
    let result = sqlx::query(
        "INSERT INTO stream_events (session_id, event_type, payload) \
         SELECT $1, $2, $3 WHERE EXISTS (SELECT 1 FROM sessions WHERE id = $1) \
         ON CONFLICT (session_id, event_type, content_hash) DO NOTHING",
    )
    .bind(local_id)
    .bind(event_type)
    .bind(payload)
    .execute(&state.pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Recursively strip NUL (`\0`) from every string in a JSON value.
/// Postgres rejects NUL in `jsonb`/`text`, so an event carrying one would be
/// dropped on insert (CCT-136). Stripping keeps the event; the NUL has no
/// display value anyway.
fn strip_nul(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::String(s) => {
            if s.contains('\0') {
                *s = s.replace('\0', "");
            }
        }
        serde_json::Value::Array(arr) => arr.iter_mut().for_each(strip_nul),
        serde_json::Value::Object(map) => map.values_mut().for_each(strip_nul),
        _ => {}
    }
}

#[allow(clippy::too_many_arguments)]
async fn insert_token_usage(
    state: &AppState,
    local_id: &str,
    message_id: &str,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_creation_tokens: u64,
) -> anyhow::Result<()> {
    // Cast u64 → i64 for sqlx; transcripts in the wild never approach
    // i64::MAX so saturating is safe.
    let i = i64::try_from(input_tokens).unwrap_or(i64::MAX);
    let o = i64::try_from(output_tokens).unwrap_or(i64::MAX);
    let cr = i64::try_from(cache_read_tokens).unwrap_or(i64::MAX);
    let cc = i64::try_from(cache_creation_tokens).unwrap_or(i64::MAX);
    sqlx::query(
        "INSERT INTO session_token_usage \
            (session_id, message_id, input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         ON CONFLICT (session_id, message_id) DO NOTHING",
    )
    .bind(local_id)
    .bind(message_id)
    .bind(i)
    .bind(o)
    .bind(cr)
    .bind(cc)
    .execute(&state.pool)
    .await?;
    Ok(())
}

async fn mark_session_ended(
    state: &AppState,
    local_id: &str,
    reason: &EndReason,
) -> anyhow::Result<()> {
    let payload = json!({ "reason": reason });
    // `WHERE EXISTS` guard: a session_ended can arrive for a session the server
    // never registered (its SessionStarted was dropped, or an ephemeral subagent
    // session) — a bare INSERT trips `stream_events_session_id_fkey`. No-op
    // cleanly instead of erroring; the UPDATE below is already missing-safe (CCT-493).
    sqlx::query(
        "INSERT INTO stream_events (session_id, event_type, payload) \
         SELECT $1, 'session_ended', $2 WHERE EXISTS (SELECT 1 FROM sessions WHERE id = $1) \
         ON CONFLICT (session_id, event_type, content_hash) DO NOTHING",
    )
    .bind(local_id)
    .bind(&payload)
    .execute(&state.pool)
    .await?;
    // Flip to the sticky terminal status `ended` so clients render the
    // terminal state IMMEDIATELY. Plain `inactive` was re-derived back to
    // Active for ~5 min from the still-recent heartbeat (admin::derive_status
    // is time-based) — masking the end of unattended/dispatched jobs. `ended`
    // is honoured as terminal by the list/search read paths regardless of
    // heartbeat age (CCT-192). We do not delete the row — archival remains the
    // persistence story; un-archive/resume can revive it.
    sqlx::query("UPDATE sessions SET status = 'ended' WHERE id = $1 AND status <> 'archived'")
        .bind(local_id)
        .execute(&state.pool)
        .await?;
    // Revoke any per-session gateway tokens (CCT-237): the session-scoped
    // cctui tokens minted at spawn map to `(session_id, account_id)` and must
    // die with the session so the gateway can no longer be driven under them.
    crate::routes::gateway::revoke_session_tokens(state, local_id).await;
    Ok(())
}

pub async fn load_reconcile(
    state: &AppState,
    machine_id: Uuid,
) -> anyhow::Result<Vec<DaemonAdapterConfig>> {
    let rows: Vec<(String, serde_json::Value, bool)> = sqlx::query_as(
        "SELECT adapter_id, config, enabled FROM adapters_enabled WHERE machine_id = $1",
    )
    .bind(machine_id)
    .fetch_all(&state.pool)
    .await?;

    // Bridge the owning user's `user_settings.data.harnessMode` into each
    // claude-code adapter's `config["mode"]` (CCT-495). The settings blob is
    // otherwise webui-only; this is the one place the server reads it. A
    // machine-level `adapters_enabled.config.mode` (if ever set) wins, so an
    // operator can still pin a machine. Codex rows are untouched.
    let harness_mode: Option<String> = sqlx::query_scalar(
        "SELECT us.data->>'harnessMode' \
         FROM machines m JOIN user_settings us ON us.user_id = m.user_id \
         WHERE m.id = $1",
    )
    .bind(machine_id)
    .fetch_optional(&state.pool)
    .await?
    .flatten();
    let adapter_mode =
        crate::routes::settings::harness_mode_to_adapter_token(harness_mode.as_deref());

    Ok(rows
        .into_iter()
        .map(|(id, mut config, enabled)| {
            if id == "claude-code" {
                // Per-machine pin wins: only inject when the row hasn't already
                // set a mode (any value — `claude-daemon`/`legacy`/bg/etc.).
                let pinned = config.get("mode").and_then(serde_json::Value::as_str).is_some();
                if !pinned {
                    if let Some(obj) = config.as_object_mut() {
                        obj.insert(
                            "mode".to_owned(),
                            serde_json::Value::String(adapter_mode.clone()),
                        );
                    } else {
                        config = serde_json::json!({ "mode": adapter_mode });
                    }
                }
            }
            DaemonAdapterConfig { adapter_id: AdapterId::new(id), config, enabled }
        })
        .collect())
}

// ---- /api/v1/users/{id}/tokens ----

#[derive(Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct MintTokenRequest {
    pub label: Option<String>,
    pub expires_at: Option<chrono::DateTime<Utc>>,
}

#[derive(serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct MintTokenResponse {
    pub token: String,
    pub label: Option<String>,
    pub expires_at: Option<chrono::DateTime<Utc>>,
}

pub async fn mint_user_token(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(user_id): Path<Uuid>,
    Json(req): Json<MintTokenRequest>,
) -> Result<Json<MintTokenResponse>, (StatusCode, Json<ApiError>)> {
    // Admin may mint for anyone; a user only for itself (CCT-410).
    let allowed = ctx.is_admin() || ctx.user_id == user_id;
    if !allowed {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ApiError { error: "cannot mint tokens for another user".into() }),
        ));
    }

    let token = user_token(&mint_secret());
    let hash = sha256_hex(&token);
    let preview = crate::auth::token_preview(&token);
    sqlx::query(
        "INSERT INTO user_tokens (user_id, token_hash, label, expires_at, token_preview) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(user_id)
    .bind(&hash)
    .bind(req.label.as_deref())
    .bind(req.expires_at)
    .bind(&preview)
    .execute(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("db error: {e}");
        (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "database error".into() }))
    })?;

    // Mirror into the unified api_keys table (CCT-410) with grant = owner's
    // ceiling, so the token behaves identically through the new auth path.
    let grant = crate::auth::ceiling_of(&state.pool, user_id).await;
    if let Err(e) = crate::auth::register_key(
        &state.pool,
        crate::auth::NewKey {
            user_id,
            key_hash: &hash,
            key_preview: Some(&preview),
            label: req.label.as_deref(),
            kind: "user",
            machine_id: None,
            dispatcher_id: None,
        },
        grant,
    )
    .await
    {
        tracing::warn!("failed to register user token in api_keys: {e}");
    }

    Ok(Json(MintTokenResponse { token, label: req.label, expires_at: req.expires_at }))
}

#[cfg(test)]
mod tests {
    use super::strip_nul;
    use serde_json::json;

    #[test]
    fn strip_nul_cleans_nested_strings() {
        let mut v = json!({
            "command": "echo hi",
            "aggregatedOutput": "ok\u{0000}bad",
            "nested": { "arr": ["a\u{0000}b", 1, true] },
        });
        strip_nul(&mut v);
        assert_eq!(v["aggregatedOutput"], "okbad");
        assert_eq!(v["nested"]["arr"][0], "ab");
        assert_eq!(v["command"], "echo hi");
        // Non-strings untouched.
        assert_eq!(v["nested"]["arr"][1], 1);
    }
}
