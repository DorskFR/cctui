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
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json, response};
use cctui_proto::adapter::{AdapterEvent, AdapterId, EndReason};
use cctui_proto::api::{ApiError, DaemonAdapterConfig, DaemonAuthRequest, DaemonAuthResponse};
use cctui_proto::chunk::{Accept, Reassembler};
use cctui_proto::ws::{DaemonFrameDown, DaemonFrameUp};
use chrono::Utc;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::auth::{AuthContext, mint_secret, sha256_hex, user_token};
use crate::state::AppState;

/// Evict a daemon whose WS yields no frame of any kind — data, ping, or pong —
/// within this window. Measured by frame arrival, not data-message completion,
/// so a slow peer still answering pings mid-transfer is not evicted; a
/// truly half-open one leaves no dead entry in the bus registry.
const DAEMON_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_mins(1);
const DAEMON_LIVENESS_CHECK: std::time::Duration = std::time::Duration::from_secs(10);

/// Bound the memory a single in-flight chunked transfer may buffer.
const MAX_TRANSFER_BYTES: usize = 64 * 1024 * 1024;

/// Drop partial chunked transfers idle past this age.
const STALE_TRANSFER: std::time::Duration = std::time::Duration::from_mins(10);

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

/// Resolve a session's gateway-routing env for the daemon's launch chokepoint.
/// The daemon calls this at every worker (re)launch — spawn, resume,
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
            whip_phrases: None,
            spawn_capability: None,
        }));
    }

    // The machine user's whip stall-phrase override rides this pull to
    // reach the connectionless `whip-stop-hook`; per-user, so it applies whether
    // or not the session is account-bound.
    let whip_phrases = resolve_whip_phrases(&state, ctx.user_id).await;

    // Resolve EVERY bound family (one account per family) and re-mint each, so a
    // worker carrying both claude + codex creds gets both restored on launch,
    // not just the last-minted family. The families emit disjoint env
    // keys, so the merge never collides.
    let accounts = crate::routes::gateway::resolve_session_accounts(&state, &session_id).await;
    if accounts.is_empty() {
        return Ok(Json(cctui_proto::api::GatewayEnvResponse {
            account_bound: false,
            env: std::collections::BTreeMap::default(),
            settings: None,
            whip_phrases,
            spawn_capability: spawn_capability_for(&state, &session_id),
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
    // Merge the bound account(s)' per-account `settings_json` so it
    // rides alongside the gateway env on this same pull. Re-served on every
    // (re)launch, it survives a daemon / claude-daemon restart; the daemon
    // deep-merges it UNDER its managed hook settings when writing the worker's
    // `--settings` file (that daemon-side merge is).
    let settings = crate::routes::gateway::resolve_session_settings(&state, &session_id).await;
    // This pull only happens when the daemon is actually (re)launching the
    // worker — a session marked `ended` (possibly by a spurious end)
    // is provably coming back to life, so un-stick the terminal status here.
    // `archived` stays parked: un-archiving is an explicit user action.
    let _ = sqlx::query("UPDATE sessions SET status = 'active' WHERE id = $1 AND status = 'ended'")
        .bind(&session_id)
        .execute(&state.pool)
        .await;
    Ok(Json(cctui_proto::api::GatewayEnvResponse {
        account_bound: true,
        env,
        settings,
        whip_phrases,
        spawn_capability: spawn_capability_for(&state, &session_id),
    }))
}

/// The session's `CctuiAgent` capability, as recorded by the spawn/dispatch that
/// launched it. `None` (including after a server restart) means the daemon
/// exposes no spawn tool to that session.
fn spawn_capability_for(
    state: &AppState,
    session_id: &str,
) -> Option<cctui_proto::api::SpawnCapability> {
    state.spawn_capabilities.get(session_id).map(|c| c.clone())
}

/// The machine user's clamped `whipStopPhrases` block from
/// `user_settings.data`, or `None` when unset / reduced to the default. Read from
/// the DB on the same gateway-env pull that carries the account settings.
async fn resolve_whip_phrases(state: &AppState, user_id: Uuid) -> Option<serde_json::Value> {
    let data: Option<serde_json::Value> =
        sqlx::query_scalar("SELECT data FROM user_settings WHERE user_id = $1")
            .bind(user_id)
            .fetch_optional(&state.pool)
            .await
            .ok()
            .flatten();
    data.as_ref().and_then(crate::routes::settings::whip_stop_phrases_of)
}

// ---- /api/v1/daemon/sessions/{id}/token-valid ----

/// Query for [`session_token_valid`]: the sha256 hex of the session token the
/// daemon launched the worker with. Hash-only on purpose — no token material
/// on the wire (invariant).
#[derive(Deserialize)]
pub struct TokenValidQuery {
    pub hash: String,
}

/// Does this session's minted token still resolve at the gateway?
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
              AND (t.expires_at IS NULL OR t.expires_at > now())
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
    headers: axum::http::HeaderMap,
) -> Result<response::Response, StatusCode> {
    let token = bearer_token(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let ctx = state.auth_config.validate(&token).await.ok_or(StatusCode::UNAUTHORIZED)?;
    let Some(machine_id) = ctx.machine_id else {
        return Err(StatusCode::FORBIDDEN);
    };
    let user_id = ctx.user_id;
    Ok(ws.on_upgrade(move |socket| handle(socket, state, machine_id, user_id)).into_response())
}

fn bearer_token(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_string)
}

enum Inbound {
    Data(String),
    Skip,
    Done,
    Idle,
}

/// One poll of the inbound WS: the next frame, or a liveness-ticker tick. Any
/// yielded frame — including a ping/pong tungstenite surfaces mid data-message —
/// refreshes `last_frame`, so liveness tracks frame arrival rather than
/// data-message completion. `stream.next()` is cancel-safe, so dropping
/// it on a ticker tick loses nothing.
async fn next_inbound<S>(
    stream: &mut S,
    last_frame: &mut tokio::time::Instant,
    liveness: &mut tokio::time::Interval,
    timeout: std::time::Duration,
) -> Inbound
where
    S: futures_util::Stream<Item = Result<Message, axum::Error>> + Unpin,
{
    tokio::select! {
        item = stream.next() => match item {
            Some(Ok(msg)) => {
                *last_frame = tokio::time::Instant::now();
                match msg {
                    Message::Text(t) => Inbound::Data(t.to_string()),
                    Message::Binary(b) => Inbound::Data(String::from_utf8_lossy(&b).to_string()),
                    Message::Close(_) => Inbound::Done,
                    _ => Inbound::Skip,
                }
            }
            Some(Err(_)) | None => Inbound::Done,
        },
        _ = liveness.tick() => {
            if last_frame.elapsed() >= timeout { Inbound::Idle } else { Inbound::Skip }
        }
    }
}

/// Feed one chunk into the connection's reassembler and produce the ack to send
/// back plus, on completion, the reassembled inner frame to process.
/// `codec` decompresses the joined payload before parsing when set.
fn handle_chunk(
    reasm: &mut Reassembler,
    transfer_id: String,
    chunk_index: u32,
    total_chunks: u32,
    data: &str,
    codec: Option<&str>,
) -> (DaemonFrameDown, Option<DaemonFrameUp>) {
    match reasm.accept(&transfer_id, chunk_index, total_chunks, data) {
        Accept::Pending(highest_contiguous_chunk) => {
            (DaemonFrameDown::ChunkAck { transfer_id, highest_contiguous_chunk }, None)
        }
        Accept::Complete(bytes) => {
            let ack = DaemonFrameDown::ChunkAck {
                transfer_id,
                highest_contiguous_chunk: total_chunks.checked_sub(1),
            };
            let joined = match codec {
                Some(codec) => match cctui_proto::compress::decompress_codec(codec, &bytes) {
                    Ok(b) => b,
                    Err(err) => {
                        tracing::warn!(%err, "reassembled chunk failed to decompress");
                        return (ack, None);
                    }
                },
                None => bytes,
            };
            match serde_json::from_slice::<DaemonFrameUp>(&joined) {
                Ok(inner) => (ack, Some(inner)),
                Err(err) => {
                    tracing::warn!(%err, "reassembled chunk payload did not parse");
                    (ack, None)
                }
            }
        }
        Accept::Restart => {
            (DaemonFrameDown::ChunkAck { transfer_id, highest_contiguous_chunk: None }, None)
        }
    }
}

/// Decode a `Compressed` envelope to its inner frame, or `None` on a
/// bad codec / base64 / payload — logged and dropped like any malformed frame.
fn decode_compressed_frame(codec: &str, data: &str) -> Option<DaemonFrameUp> {
    match cctui_proto::compress::decode_compressed(codec, data) {
        Ok(bytes) => match serde_json::from_slice::<DaemonFrameUp>(&bytes) {
            Ok(inner) => Some(inner),
            Err(err) => {
                tracing::warn!(%err, "decompressed frame did not parse");
                None
            }
        },
        Err(err) => {
            tracing::warn!(%err, "compressed frame failed to decode");
            None
        }
    }
}

/// Flatten a decoded inner frame into the leaf frames to process: a `Batch`
/// yields its events in order, anything else is a single leaf.
fn expand_batch(frame: DaemonFrameUp) -> Vec<DaemonFrameUp> {
    match frame {
        DaemonFrameUp::Batch { frames } => frames,
        other => vec![other],
    }
}

#[allow(clippy::cognitive_complexity, clippy::too_many_lines)]
async fn handle(socket: WebSocket, state: AppState, machine_id: Uuid, user_id: Uuid) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::channel::<DaemonFrameDown>(64);

    // Register the daemon for command fan-out with the bus. If a
    // stale entry exists, overwrite it (newest connection wins).
    state.bus.register_daemon(machine_id, tx.clone());
    // Replica-aware presence: record this pod as the WS owner so a
    // peer replica can forward daemon-targeted requests here.
    crate::presence::register(&state, crate::presence::Kind::Daemon, machine_id).await;

    // Send Reconcile immediately.
    match load_reconcile(&state, machine_id).await {
        Ok(adapters) => {
            let secret_scrub = load_scrub_config(&state, machine_id).await;
            if tx.send(DaemonFrameDown::Reconcile { adapters, secret_scrub }).await.is_err() {
                tracing::warn!("daemon tx closed before reconcile");
            }
        }
        Err(err) => {
            tracing::error!(%err, "load_reconcile failed");
        }
    }

    // Resume marks must follow Reconcile: the daemon needs its adapters live to
    // route the marks to before it can clamp their tail cursors.
    match load_resume_marks(&state, machine_id).await {
        Ok(session_marks) if !session_marks.is_empty() => {
            if tx.send(DaemonFrameDown::ResumeMarks { session_marks }).await.is_err() {
                tracing::warn!("daemon tx closed before resume marks");
            }
        }
        Ok(_) => {}
        Err(err) => tracing::error!(%err, "load_resume_marks failed"),
    }

    // Outbound pump. Besides forwarding `DaemonFrameDown` frames, it sends a
    // periodic WS Ping so the daemon always hears from us within its liveness
    // window. Without this, an idle connection (no commands queued) sends the
    // daemon nothing after the initial Reconcile — axum does not auto-flush a
    // Pong on the split sink while it's otherwise idle — so the daemon's
    // half-open detector tears the WS down every 60s and flaps forever.
    // The interval mirrors the daemon's 20s ping cadence and stays
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

    let mut last_frame = tokio::time::Instant::now();
    let mut liveness = tokio::time::interval(DAEMON_LIVENESS_CHECK);
    liveness.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    liveness.tick().await;
    let mut reasm = Reassembler::new(MAX_TRANSFER_BYTES);
    loop {
        reasm.evict_older_than(STALE_TRANSFER);
        let payload =
            match next_inbound(&mut stream, &mut last_frame, &mut liveness, DAEMON_READ_TIMEOUT)
                .await
            {
                Inbound::Data(payload) => payload,
                Inbound::Skip => continue,
                Inbound::Done => break,
                Inbound::Idle => {
                    tracing::warn!(%machine_id, "daemon WS idle past read timeout — evicting");
                    let count = state.eviction_tracker.record(machine_id);
                    if count >= crate::bandwidth_watch::EVICTION_THRESHOLD {
                        tracing::error!(
                            %machine_id,
                            evictions = count,
                            window_mins = crate::bandwidth_watch::EVICTION_WINDOW.as_secs() / 60,
                            "daemon WS eviction loop — machine evicted repeatedly; \
                             suspected re-upload/re-connect loop (CCT-744)",
                        );
                    }
                    break;
                }
            };
        let frame: DaemonFrameUp = match serde_json::from_str(&payload) {
            Ok(f) => f,
            Err(err) => {
                tracing::warn!(%err, "bad daemon frame");
                continue;
            }
        };
        let leaves: Vec<DaemonFrameUp> = match frame {
            DaemonFrameUp::Chunk { transfer_id, chunk_index, total_chunks, data, codec } => {
                let (ack, inner) = handle_chunk(
                    &mut reasm,
                    transfer_id,
                    chunk_index,
                    total_chunks,
                    &data,
                    codec.as_deref(),
                );
                if tx.send(ack).await.is_err() {
                    break;
                }
                match inner {
                    Some(inner) => expand_batch(inner),
                    None => continue,
                }
            }
            DaemonFrameUp::Compressed { codec, data } => {
                match decode_compressed_frame(&codec, &data) {
                    Some(inner) => expand_batch(inner),
                    None => continue,
                }
            }
            DaemonFrameUp::Batch { frames } => frames,
            other => vec![other],
        };
        for frame in leaves {
            let trace = frame_trace(&frame);
            if let Err(err) = process_frame(&state, machine_id, user_id, frame).await {
                tracing::warn!(%err, %trace, "process_frame error");
            }
        }
    }

    // Cleanup. Only drop the entry if it is STILL OURS. During a reconnect
    // race the daemon's new connection may have already overwritten the bus
    // registry with its own `tx` ("newest wins" above); an unconditional
    // remove would delete that live channel, so every command would silently
    // fail `NoDaemon` while events kept flowing (they go through
    // `process_frame`, which never touches the connection registry). The
    // bus's `unregister_daemon` applies the same-channel guard. The
    // presence row mirrors it, with its own pod guard for the cross-pod twin
    // of the same race.
    if state.bus.unregister_daemon(machine_id, &tx) {
        crate::presence::unregister(&state, crate::presence::Kind::Daemon, machine_id).await;
    }
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
        | AdapterEvent::TokenUsage { local_id, .. }
        | AdapterEvent::TranscriptMark { local_id, .. } => local_id,
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
        AdapterEvent::TranscriptMark { .. } => "transcript_mark",
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
            upsert_session(
                state,
                machine_id,
                user_id,
                &adapter_id,
                &local_id,
                None,
                None,
                None,
                None,
            )
            .await
        }
        DaemonFrameUp::Event { adapter_id, event } => {
            // Machine-scoped codex model catalog: cache it by
            // machine_id — it is not a session event and never reaches the
            // per-session handler below.
            if let AdapterEvent::CodexModels { catalog } = event {
                state.codex_catalogs.insert(machine_id, catalog);
                return Ok(());
            }
            tracing::debug!(
                %adapter_id,
                kind = event_kind(&event),
                local_id = event_local_id(&event),
                "received event",
            );
            handle_event(state, machine_id, user_id, &adapter_id, event).await
        }
        DaemonFrameUp::StageFilesResult { request_id, ok, paths, error } => {
            // Mid-chat attachment reply: fire the oneshot the
            // `POST /sessions/{id}/files` round-trip parked in the bus.
            let outcome = if ok {
                Ok(paths)
            } else {
                Err(error.unwrap_or_else(|| "daemon reported staging failure".to_owned()))
            };
            if !state.bus.resolve_stage_files(request_id, outcome) {
                tracing::debug!(%request_id, "StageFilesResult for unknown request (timed out?)");
            }
            Ok(())
        }
        DaemonFrameUp::ListDirsResult { request_id, ok, dirs, error } => {
            // Working-dir autocomplete reply: fire the oneshot the
            // `GET /machines/{id}/fs/dirs` round-trip parked in the bus.
            let outcome = if ok {
                Ok(dirs)
            } else {
                Err(error.unwrap_or_else(|| "daemon reported a listing failure".to_owned()))
            };
            if !state.bus.resolve_list_dirs(request_id, outcome) {
                tracing::debug!(%request_id, "ListDirsResult for unknown request (timed out?)");
            }
            Ok(())
        }
        DaemonFrameUp::Heartbeat { bandwidth, .. } => {
            // Machine liveness: advance `last_seen_at` on EVERY
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
            if let Some(bandwidth) = bandwidth {
                persist_bandwidth(state, machine_id, &bandwidth).await;
                detect_divergence(state, machine_id, bandwidth.event_bytes());
            }
            Ok(())
        }
        // Any future #[non_exhaustive] variants are no-ops.
        _ => Ok(()),
    }
}

/// Bump the per-machine persisted-insert counter feeding divergence detection,
/// only when a `stream_events` row was actually written.
fn note_insert(state: &AppState, machine_id: Uuid, newly_inserted: bool) {
    if newly_inserted {
        *state.machine_event_inserts.entry(machine_id).or_insert(0) += 1;
    }
}

/// Upsert the daemon's last-known per-subsystem byte counters. Fire-
/// and-forget: a failed write only loses one heartbeat's snapshot.
async fn persist_bandwidth(
    state: &AppState,
    machine_id: Uuid,
    bw: &cctui_proto::bandwidth::BandwidthSummary,
) {
    let res = sqlx::query(
        "INSERT INTO machine_bandwidth \
           (machine_id, forward, retransmit, backfill, self_update, blob_put, heartbeat, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, now()) \
         ON CONFLICT (machine_id) DO UPDATE SET \
           forward = EXCLUDED.forward, retransmit = EXCLUDED.retransmit, \
           backfill = EXCLUDED.backfill, self_update = EXCLUDED.self_update, \
           blob_put = EXCLUDED.blob_put, heartbeat = EXCLUDED.heartbeat, updated_at = now()",
    )
    .bind(machine_id)
    .bind(i64::try_from(bw.forward).unwrap_or(i64::MAX))
    .bind(i64::try_from(bw.retransmit).unwrap_or(i64::MAX))
    .bind(i64::try_from(bw.backfill).unwrap_or(i64::MAX))
    .bind(i64::try_from(bw.self_update).unwrap_or(i64::MAX))
    .bind(i64::try_from(bw.blob_put).unwrap_or(i64::MAX))
    .bind(i64::try_from(bw.heartbeat).unwrap_or(i64::MAX))
    .execute(&state.pool)
    .await;
    if let Err(err) = res {
        tracing::warn!(%err, %machine_id, "machine_bandwidth upsert failed");
    }
}

/// The 2026-07-21 failure signature: reported upload bytes climb while
/// persisted `stream_events` inserts don't. In-memory, cheap, piggybacked on the
/// heartbeat; an ERROR is the alert glitchtip forwards.
fn detect_divergence(state: &AppState, machine_id: Uuid, upload_bytes: u64) {
    let inserts = state.machine_event_inserts.get(&machine_id).map_or(0, |v| *v);
    if let Some(d) = state.divergence_tracker.observe(machine_id, upload_bytes, inserts) {
        tracing::error!(
            %machine_id,
            upload_bytes = d.upload_bytes,
            prev_upload_bytes = d.prev_upload_bytes,
            insert_count = d.insert_count,
            "upload/insert divergence — daemon uploading bytes with no new persisted \
             stream_events (CCT-744)",
        );
    }
}

/// auto-approve is scoped to tool-use permissions. `ExitPlanMode` and
/// `AskUserQuestion` are user decision points that must always be answered by
/// the user, so they are excluded from the auto-approve short-circuit even when
/// the session flag is set.
fn is_auto_approve_excluded(tool: &str) -> bool {
    tool == "ExitPlanMode" || tool == "AskUserQuestion"
}

fn should_auto_approve(tool: &str, auto_approve_enabled: bool) -> bool {
    auto_approve_enabled && !is_auto_approve_excluded(tool)
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
    // replay the whole conversation with a long visible lag. The
    // `ON CONFLICT DO NOTHING` dedup already drops the duplicate rows; gating
    // the broadcast on a real insert extends that dedup to the live stream.
    let mut newly_inserted = true;
    // Causal ordering key: stamped onto the live broadcast so it
    // matches the reload path's `seq` (both are `stream_events.id`).
    let mut inserted_seq: Option<i64> = None;
    match event {
        AdapterEvent::SessionStarted { local_id, meta } => {
            let working_dir = meta.working_dir.clone();
            let observed_at = meta.extra.get("observed_at").and_then(serde_json::Value::as_i64);
            let extra = (!meta.extra.is_null()).then(|| meta.extra.clone());
            if let Some(spawn_key) = meta.extra.get("spawn_key").and_then(serde_json::Value::as_str)
            {
                crate::routes::gateway::rebind_spawn_key(state, spawn_key, &local_id).await;
            }
            upsert_session(
                state,
                machine_id,
                user_id,
                adapter_id,
                &local_id,
                working_dir,
                meta.parent_local_id.clone(),
                observed_at,
                extra,
            )
            .await?;
        }
        AdapterEvent::Message { local_id, payload } => {
            inserted_seq = insert_event(state, &local_id, "message", payload).await?;
            newly_inserted = inserted_seq.is_some();
            note_insert(state, machine_id, newly_inserted);
        }
        AdapterEvent::ToolUse { local_id, payload } => {
            inserted_seq = insert_event(state, &local_id, "tool_use", payload).await?;
            newly_inserted = inserted_seq.is_some();
            note_insert(state, machine_id, newly_inserted);
        }
        AdapterEvent::SessionEnded { local_id, reason } => {
            mark_session_ended(state, &local_id, &reason).await?;
        }
        AdapterEvent::TranscriptMark { local_id, offset } => {
            update_transcript_mark(state, &local_id, offset).await?;
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
        AdapterEvent::Diagnose { request_id, report, .. } => {
            // Session-diagnose reply: fire the oneshot the
            // `GET /sessions/{id}/diagnose` round-trip parked in the bus. A
            // late reply (route timed out, or a spooled event replayed after
            // reconnect) resolves nothing and is dropped.
            if !state.bus.resolve_diagnose(request_id, report) {
                tracing::debug!(%request_id, "Diagnose reply for unknown request (timed out?)");
            }
        }
        AdapterEvent::PtyChunk { local_id, data } => {
            // Live terminal relay: never persisted — fan the base64
            // chunk straight out to the browsers watching this session.
            state.bus.publish_server(cctui_proto::ws::ServerEvent::PtyChunk {
                session_id: local_id,
                data,
            });
        }
        AdapterEvent::CommandResult { command_id, ok, error } => {
            // Not a session event — rebroadcast straight to clients so the
            // originating spawn request gets a definitive answer.
            if !ok {
                tracing::warn!(%command_id, ?error, "command failed on daemon");
            }
            state.bus.publish_server(cctui_proto::ws::ServerEvent::CommandResult {
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
            // Auto-approve: if the session is in auto-approve mode,
            // answer `allow` immediately without prompting any client.
            let auto_approve_enabled =
                state.permission_store.read().await.is_auto_approve(&local_id);
            if auto_approve_enabled && is_auto_approve_excluded(&tool) {
                tracing::debug!(
                    session_id = %local_id,
                    request_id = %request_id,
                    tool = %tool,
                    "skipping auto-approve for plan/ask decision prompt"
                );
            }
            if should_auto_approve(&tool, auto_approve_enabled) {
                tracing::info!(
                    session_id = %local_id,
                    request_id = %request_id,
                    tool = %tool,
                    "auto-approving permission request"
                );
                let _ = crate::bus::dispatch(
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
            state.bus.publish_server(cctui_proto::ws::ServerEvent::PermissionRequest {
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
            state.bus.publish_server(cctui_proto::ws::ServerEvent::PermissionResolved {
                session_id: local_id.clone(),
                request_id,
            });
            bump_heartbeat(state, &local_id).await;
        }
        AdapterEvent::AskQuestion { local_id, question, questions, preamble } => {
            // Live AskUserQuestion: broadcast the pending question so
            // clients render an inline prompt immediately. Ephemeral — not
            // persisted as a stream_event; the full structured tool call still
            // lands in history via the transcript once the turn advances.
            // `questions` carries the structured options so the client renders
            // the interactive form live, not just the flattened text.
            // Park it authoritatively so a client that (re)subscribes after the
            // broadcast still learns the open prompt — the broadcast alone was
            // lost forever if nobody was listening at that instant.
            state.permission_store.write().await.insert_ask(
                crate::routes::permissions::PendingAsk {
                    session_id: local_id.clone(),
                    question: question.clone(),
                    questions: questions.clone(),
                    preamble: preamble.clone(),
                    received_at: chrono::Utc::now(),
                },
            );
            state.bus.publish_server(cctui_proto::ws::ServerEvent::AskQuestion {
                session_id: local_id.clone(),
                question,
                questions,
                preamble,
            });
            bump_heartbeat(state, &local_id).await;
        }
        AdapterEvent::AskResolved { local_id } => {
            state.permission_store.write().await.remove_ask(&local_id);
            state.bus.publish_server(cctui_proto::ws::ServerEvent::AskResolved {
                session_id: local_id.clone(),
            });
            bump_heartbeat(state, &local_id).await;
        }
        AdapterEvent::PlanRequest { local_id, plan, preamble } => {
            // Live ExitPlanMode plan-approval prompt: park it
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
            state.bus.publish_server(cctui_proto::ws::ServerEvent::PlanRequest {
                session_id: local_id.clone(),
                plan,
                preamble,
            });
            bump_heartbeat(state, &local_id).await;
        }
        AdapterEvent::PlanResolved { local_id } => {
            state.permission_store.write().await.remove_plan(&local_id);
            state.bus.publish_server(cctui_proto::ws::ServerEvent::PlanResolved {
                session_id: local_id.clone(),
            });
            bump_heartbeat(state, &local_id).await;
        }
        AdapterEvent::Status {
            local_id,
            tempo,
            state: agent_state,
            detail: _,
            activity,
            name,
            intent,
            model,
            effort,
            children,
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
                    intent: intent.as_deref(),
                    model: model.as_deref(),
                    effort: effort.as_deref(),
                    children: &children,
                },
            )
            .await?;
        }
        AdapterEvent::PrLink { local_id, children } => {
            persist_pr_link_children(state, &local_id, &children).await?;
        }
        AdapterEvent::SessionModel { local_id, model } => {
            // Overwrite with the transcript/init-frame ground truth — the model
            // the session is ACTUALLY running. Previously this only
            // filled when unset, so the requested `--model` (delivered first via
            // a Status event) permanently masked a spare-claim/clamp drift. The
            // Status path now fills model only when NULL, so this ground-truth
            // write wins and sticks.
            sqlx::query("UPDATE sessions SET model = $2 WHERE id = $1")
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
    // fold the tool-activity counters into the same heartbeat write for
    // a real `ToolCall` (a tool_result normalizes to `ToolResult` → plain bump).
    let tool_name = match &broadcast_pair {
        Some((_, cctui_proto::ws::AgentEvent::ToolCall { tool, .. })) => Some(tool.as_str()),
        _ => None,
    };
    if let Some(id) = local_id_for_bump {
        // Only a *newly-inserted* tool call advances the counters: a daemon
        // replaying history on reconnect must bump the heartbeat (as it always
        // did) without re-inflating `tool_use_count` or churning `last_tool_at`.
        match tool_name {
            Some(tool) if newly_inserted => bump_tool_activity(state, &id, tool).await,
            _ => bump_heartbeat(state, &id).await,
        }
        if newly_inserted && is_user_turn(broadcast_pair.as_ref().map(|(_, e)| e)) {
            reset_tool_count(state, &id).await;
        }
    }
    if newly_inserted && let Some((session_id, mut data)) = broadcast_pair {
        if let Some(seq) = inserted_seq {
            data.set_seq(seq);
        }
        state.bus.publish_server(cctui_proto::ws::ServerEvent::Stream { session_id, data });
    }
    Ok(())
}

/// A `Message` that normalized to a non-meta user turn (`▷ User:` prefix, shared
/// by every adapter's user text) starts a new turn, so the per-turn tool count
/// resets.
fn is_user_turn(event: Option<&cctui_proto::ws::AgentEvent>) -> bool {
    matches!(
        event,
        Some(cctui_proto::ws::AgentEvent::Text { content, meta: false, .. })
            if content.starts_with("▷ User:")
    )
}

/// Latest classifier signals + display metadata from a Status event.
struct StatusSignals<'a> {
    tempo: Option<&'a str>,
    agent_state: Option<&'a str>,
    activity: Option<&'a str>,
    name: Option<&'a str>,
    intent: Option<&'a str>,
    model: Option<&'a str>,
    effort: Option<&'a str>,
    children: &'a [cctui_proto::adapter::SessionChild],
}

/// Persist the latest Status signals onto the session row. `COALESCE` keeps
/// a previously-known value when a given Status event omits a field, so a
/// sparse update never clears signal. `model` is special-cased to
/// `COALESCE(model, $6)` (fill only when NULL): the requested model must not
/// overwrite the init-frame ground truth that `SessionModel` writes.
/// `effort` is safe to overwrite because the daemon now reports the observed
/// (`/proc CLAUDE_EFFORT`) value in Status, not the requested one.
async fn update_status_signals(
    state: &AppState,
    local_id: &str,
    s: StatusSignals<'_>,
) -> anyhow::Result<()> {
    let children = serde_json::to_value(s.children).unwrap_or_else(|_| serde_json::json!([]));
    sqlx::query(
        "UPDATE sessions SET \
            tempo = COALESCE($2, tempo), \
            agent_state = COALESCE($3, agent_state), \
            activity = COALESCE($4, activity), \
            session_name = COALESCE($5, session_name), \
            intent = COALESCE($6, intent), \
            model = COALESCE(model, $7), \
            effort = COALESCE($8, effort), \
            children = CASE WHEN jsonb_array_length($9) > 0 THEN $9 ELSE children END \
         WHERE id = $1",
    )
    .bind(local_id)
    .bind(s.tempo)
    .bind(s.agent_state)
    .bind(s.activity)
    .bind(s.name)
    .bind(s.intent)
    .bind(s.model)
    .bind(s.effort)
    .bind(children)
    .execute(&state.pool)
    .await?;
    Ok(())
}

/// Fill the session row's `children` from a transcript `pr-link` line, but only
/// when it has none: an authoritative `Status` snapshot (from `state.json`) must
/// always win, so the transcript source is a gap-filler for sessions whose
/// `state.json` carries no children.
async fn persist_pr_link_children(
    state: &AppState,
    local_id: &str,
    children: &[cctui_proto::adapter::SessionChild],
) -> anyhow::Result<()> {
    if children.is_empty() {
        return Ok(());
    }
    let children = serde_json::to_value(children).unwrap_or_else(|_| serde_json::json!([]));
    sqlx::query(
        "UPDATE sessions SET children = $2 \
         WHERE id = $1 AND (children IS NULL OR children = '[]'::jsonb)",
    )
    .bind(local_id)
    .bind(children)
    .execute(&state.pool)
    .await?;
    Ok(())
}

/// Bump `last_heartbeat` for a session and, when it is a subagent, the whole
/// `parent_id` chain up to the root. A subagent's work should keep
/// its parent(s) "alive" so the parent card doesn't read idle/stale
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

/// Heartbeat bump plus the live tool-activity projection: sets
/// `last_tool_at`/`last_tool_name` for the whole `parent_id` chain (rolled up so
/// a grinding subagent freshens the parent row), and increments `tool_use_count`
/// on the leaf only (each session tracks its own per-turn count). One recursive
/// CTE round-trip — the heartbeat write, augmented, so there is no extra UPDATE
/// per tool call beyond what `bump_heartbeat` already cost.
async fn bump_tool_activity(state: &AppState, local_id: &str, tool: &str) {
    if let Err(err) = sqlx::query(
        r"WITH RECURSIVE chain AS (
            SELECT id, parent_id FROM sessions WHERE id = $1
            UNION ALL
            SELECT s.id, s.parent_id FROM sessions s JOIN chain c ON s.id = c.parent_id
        )
        UPDATE sessions SET
            last_heartbeat = now(),
            last_tool_at = now(),
            last_tool_name = $2,
            tool_use_count = CASE WHEN id = $1 THEN tool_use_count + 1 ELSE tool_use_count END
        WHERE id IN (SELECT id FROM chain)",
    )
    .bind(local_id)
    .bind(tool)
    .execute(&state.pool)
    .await
    {
        tracing::warn!(%err, %local_id, "tool-activity bump failed");
    }
}

/// Reset the leaf session's per-turn tool count on a new user prompt.
/// Leaf only: ancestors keep their own per-turn counts.
async fn reset_tool_count(state: &AppState, local_id: &str) {
    if let Err(err) = sqlx::query("UPDATE sessions SET tool_use_count = 0 WHERE id = $1")
        .bind(local_id)
        .execute(&state.pool)
        .await
    {
        tracing::warn!(%err, %local_id, "tool-count reset failed");
    }
}

#[allow(clippy::too_many_arguments)]
async fn upsert_session(
    state: &AppState,
    machine_id: Uuid,
    user_id: Uuid,
    adapter_id: &str,
    local_id: &str,
    working_dir: Option<String>,
    parent_local_id: Option<String>,
    observed_at: Option<i64>,
    extra: Option<serde_json::Value>,
) -> anyhow::Result<()> {
    // `parent_id`: resolve via a subquery rather than binding the
    // raw value so a not-yet-known parent yields NULL instead of an FK
    // violation that would drop the whole insert. In the normal case the
    // parent is upserted earlier in the same poll, so it resolves. On
    // conflict, COALESCE keeps an already-set parent and otherwise fills it
    // in from a later poll.
    //
    // `status` on conflict: the codex thread/list inventory poll
    // re-emits SessionStarted for every machine-wide thread every
    // ~15s, including ones the user archived. Preserve terminal/parked states
    // (`inactive`, `archived`, `ended`) so a re-discovery refreshes the
    // heartbeat without resurrecting the session into the Working list;
    // otherwise revive it to `active`.
    sqlx::query(
        r"INSERT INTO sessions
            (id, parent_id, account_id, machine_id, working_dir, status, registered_at,
             last_heartbeat, metadata, user_id, machine_uuid, adapter_id)
          VALUES ($1, (SELECT id FROM sessions WHERE id = $7), NULL, $2, $3, 'active',
                  COALESCE(to_timestamp($8::double precision), now()),
                  COALESCE(to_timestamp($8::double precision), now()),
                  COALESCE($9::jsonb, '{}'::jsonb), $4, $5, $6)
          ON CONFLICT (id) DO UPDATE SET
            last_heartbeat = GREATEST(sessions.last_heartbeat,
                                      COALESCE(to_timestamp($8::double precision), now())),
            status = CASE WHEN sessions.status IN ('inactive', 'archived', 'ended') THEN sessions.status ELSE 'active' END,
            adapter_id = EXCLUDED.adapter_id,
            parent_id = COALESCE(sessions.parent_id, EXCLUDED.parent_id),
            metadata = COALESCE(sessions.metadata, '{}'::jsonb) || COALESCE(EXCLUDED.metadata, '{}'::jsonb)",
    )
    .bind(local_id)
    .bind(machine_id.to_string())
    .bind(working_dir.unwrap_or_default())
    .bind(user_id)
    .bind(machine_id)
    .bind(adapter_id)
    .bind(parent_local_id)
    .bind(observed_at)
    .bind(extra)
    .execute(&state.pool)
    .await?;
    // Repair the durable account binding: the dispatch path mints the
    // gateway token BEFORE the daemon registers the session, so mint-time's
    // best-effort `UPDATE sessions SET account_id` hit no row and the binding
    // silently stayed NULL — leaving resume-after-revocation with
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

/// Insert a stream event, returning `Some(id)` (the `stream_events.id`
/// BIGSERIAL, used as the causal ordering `seq`) if a new row was
/// written and `None` if it was a duplicate suppressed by the dedup constraint
/// (or the session row was absent). Callers use the presence to decide whether
/// to broadcast the event live, so a replayed session history doesn't re-stream
/// to clients.
async fn insert_event(
    state: &AppState,
    local_id: &str,
    event_type: &str,
    mut payload: serde_json::Value,
) -> anyhow::Result<Option<i64>> {
    // Postgres jsonb/text cannot store the NUL code point (`\0`); a
    // payload carrying one (e.g. binary-ish tool output) fails the INSERT and
    // the event is silently lost. Strip NULs from every string so
    // the rest of the payload survives.
    strip_nul(&mut payload);
    // Guard the insert on the session row existing. A daemon can emit an event
    // for a session the server never registered (e.g. a session_ended whose
    // prior SessionStarted was never received, or an ephemeral subagent
    // session): a bare INSERT then trips `stream_events_session_id_fkey`,
    // spamming WARN logs and burning a failed DB round-trip per event. The
    // `WHERE EXISTS` makes that case a clean no-op (0 rows) instead — when the
    // session is present this is identical to the old insert.
    let id: Option<i64> = sqlx::query_scalar(
        "INSERT INTO stream_events (session_id, event_type, payload) \
         SELECT $1, $2, $3 WHERE EXISTS (SELECT 1 FROM sessions WHERE id = $1) \
         ON CONFLICT (session_id, event_type, content_hash) DO NOTHING \
         RETURNING id",
    )
    .bind(local_id)
    .bind(event_type)
    .bind(payload)
    .fetch_optional(&state.pool)
    .await?;
    Ok(id)
}

/// Recursively strip NUL (`\0`) from every string in a JSON value.
/// Postgres rejects NUL in `jsonb`/`text`, so an event carrying one would be
/// dropped on insert. Stripping keeps the event; the NUL has no
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
    // cleanly instead of erroring; the UPDATE below is already missing-safe.
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
    // heartbeat age. We do not delete the row — archival remains the
    // persistence story; un-archive/resume can revive it.
    sqlx::query("UPDATE sessions SET status = 'ended' WHERE id = $1 AND status <> 'archived'")
        .bind(local_id)
        .execute(&state.pool)
        .await?;
    // Revoke any per-session gateway tokens: the session-scoped
    // cctui tokens minted at spawn map to `(session_id, account_id)` and must
    // die with the session so the gateway can no longer be driven under them.
    crate::routes::gateway::revoke_session_tokens(state, local_id).await;
    Ok(())
}

/// Advance a session's stored transcript high-water mark to `offset`, keeping
/// the max so a replayed / out-of-order mark can't rewind it. Handed
/// back to the daemon as a resume point on its next connect.
async fn update_transcript_mark(
    state: &AppState,
    local_id: &str,
    offset: u64,
) -> anyhow::Result<()> {
    let offset = i64::try_from(offset).unwrap_or(i64::MAX);
    sqlx::query(
        "UPDATE sessions SET transcript_offset = GREATEST(transcript_offset, $2) WHERE id = $1",
    )
    .bind(local_id)
    .bind(offset)
    .execute(&state.pool)
    .await?;
    Ok(())
}

/// The per-session transcript high-water marks for `machine_id`,
/// handed to the daemon right after Reconcile so it resumes its tail from the
/// server's stored offset instead of replaying from zero. Only sessions with a
/// non-zero mark are returned.
pub async fn load_resume_marks(
    state: &AppState,
    machine_id: Uuid,
) -> anyhow::Result<Vec<(String, u64)>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT id, transcript_offset FROM sessions \
         WHERE machine_uuid = $1 AND transcript_offset > 0",
    )
    .bind(machine_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(rows.into_iter().map(|(id, off)| (id, u64::try_from(off).unwrap_or(0))).collect())
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
    // claude-code adapter's `config["mode"]`. The settings blob is
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

/// The effective secret-scrub config for `machine_id`'s owner: the
/// `secretScrubEnabled` flag plus the clamped `secretScrubPatterns` list from
/// `user_settings.data`, carried in every Reconcile so a running daemon applies
/// the current list without a restart. Best-effort — a DB error scrubs nothing.
pub async fn load_scrub_config(
    state: &AppState,
    machine_id: Uuid,
) -> cctui_proto::ws::SecretScrubConfig {
    let data: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT us.data FROM machines m JOIN user_settings us ON us.user_id = m.user_id \
         WHERE m.id = $1",
    )
    .bind(machine_id)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or_else(|e| {
        tracing::warn!("db error loading scrub config: {e}");
        None
    })
    .flatten();
    data.as_ref().map(crate::routes::settings::secret_scrub_of).unwrap_or_default()
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
    // Admin may mint for anyone; a user only for itself.
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

    // Mirror into the unified api_keys table with grant = owner's
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
    use std::time::Duration;

    use axum::extract::ws::Message;
    use futures_util::Stream;
    use serde_json::json;

    use cctui_proto::chunk::{Reassembler, split};
    use cctui_proto::ws::{DaemonFrameDown, DaemonFrameUp};

    use super::{
        Inbound, MAX_TRANSFER_BYTES, bearer_token, decode_compressed_frame, event_kind,
        event_local_id, expand_batch, handle_chunk, next_inbound, should_auto_approve, strip_nul,
    };

    async fn drive<S>(mut stream: S, timeout: Duration, check: Duration) -> Inbound
    where
        S: Stream<Item = Result<Message, axum::Error>> + Unpin,
    {
        let mut last = tokio::time::Instant::now();
        let mut liveness = tokio::time::interval(check);
        liveness.tick().await;
        loop {
            match next_inbound(&mut stream, &mut last, &mut liveness, timeout).await {
                Inbound::Data(_) | Inbound::Skip => {}
                term @ (Inbound::Done | Inbound::Idle) => return term,
            }
        }
    }

    #[tokio::test]
    async fn half_open_peer_is_evicted() {
        let stream = futures_util::stream::pending::<Result<Message, axum::Error>>();
        let out = drive(stream, Duration::from_millis(300), Duration::from_millis(25)).await;
        assert!(matches!(out, Inbound::Idle));
    }

    #[tokio::test]
    async fn slow_trickle_answering_pings_survives() {
        let stream = futures_util::stream::unfold(0u32, |i| async move {
            if i >= 12 {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
            Some((Ok::<_, axum::Error>(Message::Pong(Vec::new().into())), i + 1))
        });
        let stream = Box::pin(stream);
        let out = drive(stream, Duration::from_millis(300), Duration::from_millis(25)).await;
        assert!(matches!(out, Inbound::Done));
    }

    #[test]
    fn bearer_token_reads_authorization_header() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            axum::http::HeaderValue::from_static("Bearer mkey-123"),
        );
        assert_eq!(bearer_token(&headers).as_deref(), Some("mkey-123"));
        assert!(bearer_token(&axum::http::HeaderMap::new()).is_none());
    }

    #[test]
    fn ws_auth_is_header_only() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            axum::http::HeaderValue::from_static("Bearer header-key"),
        );
        assert_eq!(bearer_token(&headers).as_deref(), Some("header-key"));
        assert!(bearer_token(&axum::http::HeaderMap::new()).is_none());
    }

    #[test]
    fn auto_approve_excludes_plan_and_ask_but_allows_tools() {
        assert!(should_auto_approve("Bash", true));
        assert!(should_auto_approve("Edit", true));
        assert!(should_auto_approve("Write", true));
        assert!(!should_auto_approve("ExitPlanMode", true));
        assert!(!should_auto_approve("AskUserQuestion", true));
    }

    #[test]
    fn auto_approve_off_never_approves() {
        assert!(!should_auto_approve("Bash", false));
        assert!(!should_auto_approve("ExitPlanMode", false));
    }

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

    #[test]
    fn transcript_mark_event_is_recognized_and_routed() {
        let ev = cctui_proto::adapter::AdapterEvent::TranscriptMark {
            local_id: "sess-9".into(),
            offset: 4096,
        };
        assert_eq!(event_kind(&ev), "transcript_mark");
        assert_eq!(event_local_id(&ev), "sess-9");
    }

    #[test]
    fn resume_marks_frame_carries_stored_offsets() {
        let rows: Vec<(String, u64)> = vec![("sess-a".into(), 4096), ("sess-b".into(), 12)];
        let frame = DaemonFrameDown::ResumeMarks { session_marks: rows.clone() };
        let json = serde_json::to_string(&frame).unwrap();
        assert!(json.contains(r#""type":"resume_marks""#));
        match serde_json::from_str::<DaemonFrameDown>(&json).unwrap() {
            DaemonFrameDown::ResumeMarks { session_marks } => assert_eq!(session_marks, rows),
            _ => panic!("expected ResumeMarks"),
        }
    }

    fn big_event(local_id: &str, filler: char) -> (DaemonFrameUp, Vec<DaemonFrameUp>) {
        let event = DaemonFrameUp::Event {
            adapter_id: "claude-code".into(),
            event: cctui_proto::adapter::AdapterEvent::Message {
                local_id: local_id.into(),
                payload: json!({ "text": filler.to_string().repeat(600 * 1024) }),
            },
        };
        let bytes = serde_json::to_vec(&event).unwrap();
        let chunks = split(&bytes).expect("payload must exceed the chunk threshold");
        (event, chunks)
    }

    fn feed(
        reasm: &mut Reassembler,
        frame: &DaemonFrameUp,
    ) -> (DaemonFrameDown, Option<DaemonFrameUp>) {
        let DaemonFrameUp::Chunk { transfer_id, chunk_index, total_chunks, data, codec } = frame
        else {
            panic!("not a chunk frame");
        };
        handle_chunk(
            reasm,
            transfer_id.clone(),
            *chunk_index,
            *total_chunks,
            data,
            codec.as_deref(),
        )
    }

    #[test]
    fn chunk_ack_reports_highest_contiguous_prefix() {
        let (_event, chunks) = big_event("s1", 'a');
        assert!(chunks.len() >= 3);
        let mut reasm = Reassembler::new(MAX_TRANSFER_BYTES);

        let (ack, inner) = feed(&mut reasm, &chunks[0]);
        assert!(inner.is_none());
        assert!(matches!(ack, DaemonFrameDown::ChunkAck { highest_contiguous_chunk: Some(0), .. }));

        // A gap at chunk 1 keeps the acked prefix at 0 even after chunk 2 lands.
        let (ack, _) = feed(&mut reasm, &chunks[2]);
        assert!(matches!(ack, DaemonFrameDown::ChunkAck { highest_contiguous_chunk: Some(0), .. }));
    }

    #[test]
    fn completed_transfer_yields_the_original_inner_frame() {
        let (event, chunks) = big_event("s1", 'b');
        let mut reasm = Reassembler::new(MAX_TRANSFER_BYTES);
        let mut recovered = None;
        for c in &chunks {
            if let (_, Some(inner)) = feed(&mut reasm, c) {
                recovered = Some(inner);
            }
        }
        let recovered = recovered.expect("transfer never completed");
        assert_eq!(
            serde_json::to_value(&recovered).unwrap(),
            serde_json::to_value(&event).unwrap(),
        );
        assert!(reasm.is_empty(), "completed transfer is dropped from the buffer");
    }

    #[test]
    fn interleaved_transfers_reassemble_independently() {
        let (ev_a, ca) = big_event("s-a", 'a');
        let (ev_b, cb) = big_event("s-b", 'b');
        assert_eq!(ca.len(), cb.len());
        let mut reasm = Reassembler::new(MAX_TRANSFER_BYTES);
        let mut done = vec![];
        for i in 0..ca.len() {
            if let (_, Some(inner)) = feed(&mut reasm, &ca[i]) {
                done.push(serde_json::to_value(&inner).unwrap());
            }
            if let (_, Some(inner)) = feed(&mut reasm, &cb[i]) {
                done.push(serde_json::to_value(&inner).unwrap());
            }
        }
        assert_eq!(
            done,
            vec![serde_json::to_value(&ev_a).unwrap(), serde_json::to_value(&ev_b).unwrap()],
        );
        assert!(reasm.is_empty());
    }

    #[test]
    fn no_usable_prefix_nacks_restart() {
        let (_event, chunks) = big_event("s1", 'c');
        let mut reasm = Reassembler::new(MAX_TRANSFER_BYTES);
        // Chunk 1 before chunk 0: nothing contiguous yet, so the daemon restarts.
        let (ack, inner) = feed(&mut reasm, &chunks[1]);
        assert!(inner.is_none());
        assert!(matches!(ack, DaemonFrameDown::ChunkAck { highest_contiguous_chunk: None, .. }));
    }

    #[test]
    fn stale_buffers_are_evicted() {
        let (_event, chunks) = big_event("s1", 'd');
        let mut reasm = Reassembler::new(MAX_TRANSFER_BYTES);
        let _ = feed(&mut reasm, &chunks[0]);
        assert_eq!(reasm.len(), 1);
        reasm.evict_older_than(Duration::ZERO);
        assert!(reasm.is_empty(), "the server drops partial transfers past the stale age");
    }

    fn event(local_id: &str) -> DaemonFrameUp {
        DaemonFrameUp::Event {
            adapter_id: "claude-code".into(),
            event: cctui_proto::adapter::AdapterEvent::Message {
                local_id: local_id.into(),
                payload: json!({ "text": "x".repeat(8 * 1024) }),
            },
        }
    }

    #[test]
    fn decodes_legacy_plain_frame() {
        // An old daemon's plain Event (no compression/batching) is a single leaf.
        let leaves = expand_batch(event("s1"));
        assert_eq!(leaves.len(), 1);
        assert!(matches!(leaves[0], DaemonFrameUp::Event { .. }));
    }

    #[test]
    fn decodes_compressed_frame() {
        let inner = event("s1");
        let json = serde_json::to_vec(&inner).unwrap();
        let compressed = cctui_proto::compress::zstd_compress(&json);
        let DaemonFrameUp::Compressed { codec, data } =
            cctui_proto::compress::compressed_frame("zstd", &compressed)
        else {
            panic!("compressed_frame must build a Compressed");
        };
        let decoded = decode_compressed_frame(&codec, &data).expect("must decode");
        assert_eq!(serde_json::to_value(&decoded).unwrap(), serde_json::to_value(&inner).unwrap());
    }

    #[test]
    fn decodes_batch_frame_in_order() {
        let batch =
            DaemonFrameUp::Batch { frames: (0..4).map(|i| event(&format!("s{i}"))).collect() };
        let leaves = expand_batch(batch);
        assert_eq!(leaves.len(), 4);
    }

    #[test]
    fn bad_compressed_frame_is_dropped() {
        assert!(decode_compressed_frame("zstd", "!!not base64!!").is_none());
        assert!(decode_compressed_frame("brotli", "AAAA").is_none());
    }

    #[test]
    fn decodes_chunked_compressed_batch() {
        // Full compose: a batch too big for one message, compressed then chunked.
        // The server reassembles, decompresses via the chunk codec, parses the
        // inner Batch, and expands it back to its events.
        let mut rng = 0x1234_5678_9abc_def0_u64;
        let mut blob = |n: usize| {
            let bytes: Vec<u8> = (0..n)
                .map(|_| {
                    rng ^= rng << 13;
                    rng ^= rng >> 7;
                    rng ^= rng << 17;
                    rng as u8
                })
                .collect();
            hex::encode(bytes)
        };
        let frames: Vec<DaemonFrameUp> = (0..80)
            .map(|i| DaemonFrameUp::Event {
                adapter_id: "claude-code".into(),
                event: cctui_proto::adapter::AdapterEvent::Message {
                    local_id: format!("s{i}"),
                    payload: json!({ "n": i, "blob": blob(4000) }),
                },
            })
            .collect();
        let want = frames.len();
        let inner = DaemonFrameUp::Batch { frames };
        let serialized = serde_json::to_vec(&inner).unwrap();
        let compressed = cctui_proto::compress::zstd_compress(&serialized);
        let id = cctui_proto::chunk::transfer_id(&compressed);
        let total = cctui_proto::chunk::chunk_count(compressed.len());
        assert!(total > 1, "compressed batch must span multiple chunks");

        let mut reasm = Reassembler::new(MAX_TRANSFER_BYTES);
        let mut recovered = None;
        for i in 0..total {
            let frame = cctui_proto::chunk::chunk_frame(&id, &compressed, i, total, Some("zstd"));
            if let (_, Some(inner)) = feed(&mut reasm, &frame) {
                recovered = Some(inner);
            }
        }
        let leaves = expand_batch(recovered.expect("transfer completed"));
        assert_eq!(leaves.len(), want, "chunked+compressed batch expands to its events");
    }
}
