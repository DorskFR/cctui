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

use crate::auth::{AuthContext, TokenRole, mint_secret, sha256_hex, user_token};
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
    if ctx.role != TokenRole::Machine {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ApiError { error: "machine token required".into() }),
        ));
    }
    let (Some(machine_id), Some(user_id)) = (ctx.machine_id, ctx.user_id) else {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError { error: "machine context missing ids".into() }),
        ));
    };
    Ok(Json(DaemonAuthResponse {
        session_token: req.machine_key,
        expires_at: Utc::now() + chrono::Duration::hours(24),
        machine_id,
        user_id,
    }))
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
    if ctx.role != TokenRole::Machine {
        return Err(StatusCode::FORBIDDEN);
    }
    let machine_id = ctx.machine_id.ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    let user_id = ctx.user_id.ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
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

    // Cleanup.
    state.daemon_connections.remove(&machine_id);
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
        _ => "other",
    }
}

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
        // Heartbeat (and any future #[non_exhaustive] variants) are no-ops.
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
            insert_event(state, &local_id, "message", payload).await?;
        }
        AdapterEvent::ToolUse { local_id, payload } => {
            insert_event(state, &local_id, "tool_use", payload).await?;
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
        _ => {}
    }
    if let Some(id) = local_id_for_bump {
        bump_heartbeat(state, &id).await;
    }
    if let Some((session_id, data)) = broadcast_pair {
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

async fn bump_heartbeat(state: &AppState, local_id: &str) {
    if let Err(err) = sqlx::query("UPDATE sessions SET last_heartbeat = now() WHERE id = $1")
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
    sqlx::query(
        r"INSERT INTO sessions
            (id, parent_id, account_id, machine_id, working_dir, status, registered_at,
             last_heartbeat, metadata, user_id, machine_uuid, adapter_id)
          VALUES ($1, (SELECT id FROM sessions WHERE id = $7), NULL, $2, $3, 'active',
                  now(), now(), '{}'::jsonb, $4, $5, $6)
          ON CONFLICT (id) DO UPDATE SET
            last_heartbeat = now(),
            status = CASE WHEN sessions.status = 'inactive' THEN 'inactive' ELSE 'active' END,
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
    Ok(())
}

async fn insert_event(
    state: &AppState,
    local_id: &str,
    event_type: &str,
    mut payload: serde_json::Value,
) -> anyhow::Result<()> {
    // Postgres jsonb/text cannot store the NUL code point (` `); a
    // payload carrying one (e.g. binary-ish tool output) fails the INSERT and
    // the event is silently lost (CCT-136). Strip NULs from every string so
    // the rest of the payload survives.
    strip_nul(&mut payload);
    sqlx::query(
        "INSERT INTO stream_events (session_id, event_type, payload) VALUES ($1, $2, $3) \
         ON CONFLICT (session_id, event_type, content_hash) DO NOTHING",
    )
    .bind(local_id)
    .bind(event_type)
    .bind(payload)
    .execute(&state.pool)
    .await?;
    Ok(())
}

/// Recursively strip NUL (` `) from every string in a JSON value.
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
    sqlx::query(
        "INSERT INTO stream_events (session_id, event_type, payload) \
         VALUES ($1, 'session_ended', $2) \
         ON CONFLICT (session_id, event_type, content_hash) DO NOTHING",
    )
    .bind(local_id)
    .bind(&payload)
    .execute(&state.pool)
    .await?;
    // Flip status to inactive so clients render the terminal state. We do
    // not delete the row — archival remains the persistence story.
    sqlx::query("UPDATE sessions SET status = 'inactive' WHERE id = $1")
        .bind(local_id)
        .execute(&state.pool)
        .await?;
    Ok(())
}

async fn load_reconcile(
    state: &AppState,
    machine_id: Uuid,
) -> anyhow::Result<Vec<DaemonAdapterConfig>> {
    let rows: Vec<(String, serde_json::Value, bool)> = sqlx::query_as(
        "SELECT adapter_id, config, enabled FROM adapters_enabled WHERE machine_id = $1",
    )
    .bind(machine_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, config, enabled)| DaemonAdapterConfig {
            adapter_id: AdapterId::new(id),
            config,
            enabled,
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
    let allowed = match (ctx.role, ctx.user_id) {
        (TokenRole::Admin, _) => true,
        (TokenRole::User, Some(uid)) => uid == user_id,
        _ => false,
    };
    if !allowed {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ApiError { error: "cannot mint tokens for another user".into() }),
        ));
    }

    let token = user_token(&mint_secret());
    let hash = sha256_hex(&token);
    sqlx::query(
        "INSERT INTO user_tokens (user_id, token_hash, label, expires_at) VALUES ($1, $2, $3, $4)",
    )
    .bind(user_id)
    .bind(&hash)
    .bind(req.label.as_deref())
    .bind(req.expires_at)
    .execute(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("db error: {e}");
        (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "database error".into() }))
    })?;

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
