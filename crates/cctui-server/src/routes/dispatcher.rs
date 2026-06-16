//! Enrolled-dispatcher ↔ Server contract surface (CCT-285, finishing CCT-248).
//!
//! Peer of [`crate::routes::daemon`]. A standalone executor service
//! (cctui-dispatcher-kube / -docker) enrolls once, then dials out:
//!   * `POST /api/v1/dispatcher/enroll` — user token mints a dispatcher
//!     identity row (id, kind, enrollment key). Key returned ONCE.
//!   * `POST /api/v1/dispatcher/auth`   — dispatcher key → confirms identity.
//!   * `GET  /api/v1/dispatcher/ws`     — long-lived bidirectional WS. The
//!     dispatcher sends [`DispatcherFrameUp`]; the server sends
//!     [`DispatcherFrameDown`] (Dispatch/Status/Cancel).
//!
//! The server only forwards a [`cctui_proto::ws::WireDispatchSpec`]; the
//! executor binary lifts the machine key / payload semantics on its side.

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{State, WebSocketUpgrade};
use axum::http::{StatusCode, Uri};
use axum::response::IntoResponse;
use axum::{Extension, Json, response};
use cctui_proto::api::{ApiError, DaemonAuthRequest, DaemonAuthResponse};
use cctui_proto::ws::{DispatcherFrameDown, DispatcherFrameUp};
use chrono::Utc;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::auth::{
    AuthContext, machine_token, mint_secret, require_user, sha256_hex, token_preview,
};
use crate::state::AppState;

/// Evict a dispatcher whose WS produced no frame within this window. A healthy
/// dispatcher heartbeats every ~20s, so 3× that distinguishes a half-open
/// connection from idleness (mirrors the daemon path, CCT-140).
const DISPATCHER_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

// ---- /api/v1/dispatcher/enroll ----

#[derive(Deserialize)]
pub struct EnrollRequest {
    pub name: String,
    /// Reported by the binary: `kubernetes` | `docker` | `http`. Free-form;
    /// recorded as-is for display/liveness only — the server never branches on
    /// it (the executor owns runtime semantics).
    #[serde(default)]
    pub kind: Option<String>,
}

#[derive(Serialize)]
pub struct EnrollResponse {
    pub dispatcher_id: Uuid,
    pub dispatcher_key: String,
    pub server_version: &'static str,
}

/// `POST /api/v1/dispatcher/enroll` — user token mints a dispatcher identity.
/// The key is returned ONCE and only its hash + a preview are persisted (same
/// discipline as the machine enroll).
pub async fn enroll(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<EnrollRequest>,
) -> Result<Json<EnrollResponse>, (StatusCode, Json<ApiError>)> {
    let user_id = require_user(&ctx)
        .map_err(|s| (s, Json(ApiError { error: "user token required".into() })))?;

    if req.name.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, Json(ApiError { error: "name required".into() })));
    }

    let dispatcher_id = Uuid::new_v4();
    let secret = mint_secret();
    let token = machine_token(&secret);
    let key_hash = sha256_hex(&token);
    let kind = req.kind.as_deref().filter(|k| !k.trim().is_empty()).unwrap_or("http");

    sqlx::query(
        "INSERT INTO dispatchers (id, user_id, name, kind, key_hash, key_preview) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(dispatcher_id)
    .bind(user_id)
    .bind(req.name.trim())
    .bind(kind)
    .bind(&key_hash)
    .bind(token_preview(&token))
    .execute(&state.pool)
    .await
    .map_err(|e| {
        if let sqlx::Error::Database(dbe) = &e
            && dbe.code().as_deref() == Some("23505")
        {
            return (
                StatusCode::CONFLICT,
                Json(ApiError { error: "a dispatcher with that name already exists".into() }),
            );
        }
        tracing::error!("db error: {e}");
        (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "database error".into() }))
    })?;

    tracing::info!(%user_id, %dispatcher_id, name = %req.name, kind, "dispatcher enrolled");

    Ok(Json(EnrollResponse {
        dispatcher_id,
        dispatcher_key: token,
        server_version: env!("CARGO_PKG_VERSION"),
    }))
}

// ---- /api/v1/dispatcher/auth ----

/// `POST /api/v1/dispatcher/auth` — the dispatcher presents its enrollment key
/// up-front so a misconfiguration fails loudly before the WS loop. Reuses the
/// daemon auth shapes; the dispatcher id is returned in `machine_id`.
pub async fn auth(
    State(state): State<AppState>,
    Json(req): Json<DaemonAuthRequest>,
) -> Result<Json<DaemonAuthResponse>, (StatusCode, Json<ApiError>)> {
    let (dispatcher_id, user_id) =
        resolve_dispatcher_key(&state, &req.machine_key).await.ok_or_else(|| {
            (StatusCode::UNAUTHORIZED, Json(ApiError { error: "invalid dispatcher key".into() }))
        })?;
    Ok(Json(DaemonAuthResponse {
        session_token: req.machine_key,
        expires_at: Utc::now() + chrono::Duration::hours(24),
        machine_id: dispatcher_id,
        user_id,
    }))
}

// ---- /api/v1/dispatcher/ws ----

pub async fn ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    uri: Uri,
) -> Result<response::Response, StatusCode> {
    let token = extract_token_from_uri(&uri).ok_or(StatusCode::UNAUTHORIZED)?;
    let (dispatcher_id, _user_id) =
        resolve_dispatcher_key(&state, &token).await.ok_or(StatusCode::UNAUTHORIZED)?;
    Ok(ws.on_upgrade(move |socket| handle(socket, state, dispatcher_id)).into_response())
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

/// Resolve a dispatcher enrollment key to `(dispatcher_id, user_id)`. A
/// dispatcher key is NOT a machine key, so it does not flow through
/// [`crate::auth::AuthConfig::validate`] — it resolves against the
/// `dispatchers` table directly. Revoked dispatchers and disabled/revoked
/// owners are rejected.
async fn resolve_dispatcher_key(state: &AppState, key: &str) -> Option<(Uuid, Uuid)> {
    let hash = sha256_hex(key);
    sqlx::query_as::<_, (Uuid, Uuid)>(
        "SELECT d.id, d.user_id FROM dispatchers d \
         JOIN users u ON u.id = d.user_id \
         WHERE d.key_hash = $1 AND d.revoked_at IS NULL AND d.deleted_at IS NULL \
         AND u.revoked_at IS NULL AND u.disabled_at IS NULL",
    )
    .bind(&hash)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten()
}

#[allow(clippy::cognitive_complexity)]
async fn handle(socket: WebSocket, state: AppState, dispatcher_id: Uuid) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::channel::<DispatcherFrameDown>(64);

    // Newest connection wins (mirrors the daemon hub).
    state.dispatcher_connections.insert(dispatcher_id, tx.clone());
    bump_last_seen(&state, dispatcher_id).await;
    crate::machine_liveness::record_and_broadcast_dispatcher(
        &state,
        dispatcher_id,
        cctui_proto::models::MachineLiveness::Online,
    );

    // Outbound pump: forward DispatcherFrameDown frames + a periodic Ping so the
    // dispatcher always hears from us within its liveness window (CCT-144).
    let outbound = tokio::spawn(async move {
        let mut keepalive = tokio::time::interval(std::time::Duration::from_secs(20));
        keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        keepalive.tick().await;
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

    // Inbound loop.
    loop {
        let msg = match tokio::time::timeout(DISPATCHER_READ_TIMEOUT, stream.next()).await {
            Ok(Some(Ok(msg))) => msg,
            Ok(Some(Err(_)) | None) => break,
            Err(_) => {
                tracing::warn!(%dispatcher_id, "dispatcher WS idle past read timeout — evicting");
                break;
            }
        };
        let payload = match msg {
            Message::Text(t) => t.to_string(),
            Message::Binary(b) => String::from_utf8_lossy(&b).to_string(),
            Message::Close(_) => break,
            _ => continue,
        };
        let frame: DispatcherFrameUp = match serde_json::from_str(&payload) {
            Ok(f) => f,
            Err(err) => {
                tracing::warn!(%err, "bad dispatcher frame");
                continue;
            }
        };
        process_frame(&state, dispatcher_id, frame).await;
    }

    // Cleanup: only drop the entry if it is STILL OURS (reconnect race, CCT-159).
    state.dispatcher_connections.remove_if(&dispatcher_id, |_, current| current.same_channel(&tx));
    outbound.abort();
}

async fn process_frame(state: &AppState, dispatcher_id: Uuid, frame: DispatcherFrameUp) {
    match frame {
        DispatcherFrameUp::Hello { kind, version } => {
            tracing::info!(%dispatcher_id, %kind, %version, "dispatcher hello");
            bump_last_seen(state, dispatcher_id).await;
            crate::machine_liveness::record_and_broadcast_dispatcher(
                state,
                dispatcher_id,
                cctui_proto::models::MachineLiveness::Online,
            );
        }
        DispatcherFrameUp::Heartbeat { .. } => {
            bump_last_seen(state, dispatcher_id).await;
            crate::machine_liveness::record_and_broadcast_dispatcher(
                state,
                dispatcher_id,
                cctui_proto::models::MachineLiveness::Online,
            );
        }
        // Every reply carries the request_id the dispatch path is awaiting; fire
        // the matching oneshot. An unknown id means the route already timed out.
        DispatcherFrameUp::DispatchResult { request_id, .. }
        | DispatcherFrameUp::StatusResult { request_id, .. }
        | DispatcherFrameUp::CancelResult { request_id, .. } => {
            if let Some((_, reply_tx)) = state.pending_dispatcher_requests.remove(&request_id) {
                let _ = reply_tx.send(frame);
            } else {
                tracing::debug!(%request_id, "dispatcher reply for unknown request (timed out?)");
            }
        }
        // Future #[non_exhaustive] variants are no-ops.
        _ => {}
    }
}

async fn bump_last_seen(state: &AppState, dispatcher_id: Uuid) {
    if let Err(err) = sqlx::query("UPDATE dispatchers SET last_seen_at = now() WHERE id = $1")
        .bind(dispatcher_id)
        .execute(&state.pool)
        .await
    {
        tracing::warn!(%err, %dispatcher_id, "dispatcher last_seen_at bump failed");
    }
}
