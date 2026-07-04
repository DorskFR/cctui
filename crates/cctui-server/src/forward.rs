//! Cross-replica request forwarding (CCT-567).
//!
//! With multiple server replicas, a daemon/dispatcher WS is terminated by
//! exactly one pod while HTTP requests load-balance across all of them. Routes
//! that need a live WS call the `ensure_*_local` guards early: if the
//! connection isn't in this pod's in-memory registry but [`crate::presence`]
//! says a live peer owns it, the guard answers `421 Misdirected Request` with
//! the owner's IP encoded in the body. [`forward_mw`] — layered on exactly the
//! WS-targeted routes — catches that 421 and transparently re-sends the
//! original request (same path, headers, body, Authorization) to the owning
//! pod, which authenticates it like any other request.
//!
//! Loop guard: forwarded requests carry [`FORWARDED_HEADER`] and are never
//! re-forwarded; a 421 coming back from a peer (the WS moved again mid-flight)
//! surfaces as 502 rather than bouncing around the mesh.

use axum::Json;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use cctui_proto::api::ApiError;
use uuid::Uuid;

use crate::presence::{self, Kind};
use crate::state::AppState;

/// Marks a request as already forwarded once; the receiving pod never
/// re-forwards it.
pub const FORWARDED_HEADER: &str = "x-cctui-forwarded";

/// Body prefix carrying the owning peer's IP inside the 421 [`ApiError`]. The
/// handlers' error type is `(StatusCode, Json<ApiError>)` (no header slot), so
/// the owner rides the body; only [`forward_mw`] ever sees the 421 — it either
/// forwards or rewrites it, so the marker never reaches a client.
const NOT_LOCAL_PREFIX: &str = "ws-owner:";

/// How long a forwarded request may take end to end on the peer. Must exceed
/// the longest in-handler WS round-trip (dispatcher dispatch / file staging,
/// 30s each) with headroom for the spawn body transfer.
const FORWARD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(75);

/// Largest request body [`forward_mw`] will buffer. The routes it is layered
/// on cap at 24 MB (`DefaultBodyLimit`); this is a backstop, not a policy.
const MAX_BUFFERED_BODY: usize = 32 * 1024 * 1024;

/// The 421 a WS-targeted handler returns when a live peer owns the target
/// connection. Consumed exclusively by [`forward_mw`].
pub fn not_local_error(owner_ip: &str) -> (StatusCode, Json<ApiError>) {
    (
        StatusCode::MISDIRECTED_REQUEST,
        Json(ApiError { error: format!("{NOT_LOCAL_PREFIX}{owner_ip}") }),
    )
}

/// Extract + validate the peer IP from a 421 body produced by
/// [`not_local_error`]. Strict `IpAddr` parsing so a forged/garbled body can
/// never steer the forwarder at an arbitrary host.
fn parse_not_local(body: &[u8]) -> Option<String> {
    let err: ApiError = serde_json::from_slice(body).ok()?;
    let ip = err.error.strip_prefix(NOT_LOCAL_PREFIX)?;
    ip.parse::<std::net::IpAddr>().ok().map(|a| a.to_string())
}

/// Guard for machine-addressed routes: `Ok` when this pod holds the daemon WS
/// (or nobody provably does — the caller's existing offline handling applies);
/// `Err(421)` when a live peer owns it.
pub async fn ensure_daemon_local(
    state: &AppState,
    machine_uuid: Uuid,
) -> Result<(), (StatusCode, Json<ApiError>)> {
    if state.daemon_connections.contains_key(&machine_uuid) {
        return Ok(());
    }
    presence::peer_owner(state, Kind::Daemon, machine_uuid)
        .await
        .map_or(Ok(()), |owner| Err(not_local_error(&owner)))
}

/// [`ensure_daemon_local`] for session-addressed routes: resolves the session's
/// `machine_uuid` first. A session with no machine (or an unknown id) passes —
/// the handler's own not-found/no-machine handling stays authoritative.
pub async fn ensure_session_daemon_local(
    state: &AppState,
    session_id: &str,
) -> Result<(), (StatusCode, Json<ApiError>)> {
    let machine: Option<Option<Uuid>> =
        sqlx::query_scalar("SELECT machine_uuid FROM sessions WHERE id = $1")
            .bind(session_id)
            .fetch_optional(&state.pool)
            .await
            .map_err(|e| {
                tracing::error!("db error (locality lookup): {e}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiError { error: "database error".into() }),
                )
            })?;
    match machine.flatten() {
        Some(machine_uuid) => ensure_daemon_local(state, machine_uuid).await,
        None => Ok(()),
    }
}

/// Whether a request path is one of the WS-targeted routes the forwarder
/// participates in. Everything else passes through [`forward_mw`] unbuffered —
/// in particular the streaming archive/skills uploads must never be buffered.
/// Matched on trailing segments so it holds with or without the `/api/v1` nest
/// prefix.
fn is_ws_targeted(path: &str) -> bool {
    let segs: Vec<&str> = path.trim_matches('/').split('/').collect();
    match segs.as_slice() {
        [.., "sessions", "dispatch" | "spawn"] => true,
        [.., "sessions", _id, action] => matches!(
            *action,
            "message"
                | "kill"
                | "interrupt"
                | "resume"
                | "set-model"
                | "switch-account"
                | "fork"
                | "files"
                | "launch"
        ),
        [.., "machines", _id, "fs", "dirs"] => true,
        _ => false,
    }
}

/// Middleware layered on the API router: for WS-targeted routes, buffers the
/// (bounded) request body, runs the route, and on a 421 re-sends the original
/// request to the owning peer pod, returning the peer's response verbatim.
pub async fn forward_mw(State(state): State<AppState>, req: Request, next: Next) -> Response {
    // Peer side of a forward — or a hop already taken — or a route the
    // forwarder doesn't participate in: run straight through, unbuffered.
    if req.headers().contains_key(FORWARDED_HEADER) || !is_ws_targeted(req.uri().path()) {
        return next.run(req).await;
    }

    let (parts, body) = req.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, MAX_BUFFERED_BODY).await else {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(ApiError { error: "request body too large".into() }),
        )
            .into_response();
    };

    let rebuilt = Request::from_parts(parts.clone(), axum::body::Body::from(bytes.clone()));
    let response = next.run(rebuilt).await;
    if response.status() != StatusCode::MISDIRECTED_REQUEST {
        return response;
    }

    // Our own 421: extract the owner and forward. Any parse failure means the
    // 421 wasn't ours — pass it through untouched.
    let (res_parts, res_body) = response.into_parts();
    let Ok(res_bytes) = axum::body::to_bytes(res_body, 64 * 1024).await else {
        return (
            StatusCode::BAD_GATEWAY,
            Json(ApiError { error: "unreadable not-local response".into() }),
        )
            .into_response();
    };
    let Some(owner) = parse_not_local(&res_bytes) else {
        return Response::from_parts(res_parts, axum::body::Body::from(res_bytes));
    };

    let path_and_query =
        parts.uri.path_and_query().map_or_else(|| parts.uri.path(), |pq| pq.as_str());
    // IPv6 literals need bracketing in authority position.
    let host = if owner.contains(':') { format!("[{owner}]") } else { owner.clone() };
    let url = format!("http://{host}:{}{path_and_query}", state.config.port);
    tracing::info!(%url, method = %parts.method, "forwarding WS-targeted request to owning peer");

    let Ok(method) = reqwest::Method::from_bytes(parts.method.as_str().as_bytes()) else {
        return (StatusCode::BAD_GATEWAY, Json(ApiError { error: "unforwardable method".into() }))
            .into_response();
    };
    let mut builder =
        state.http_client.request(method, &url).timeout(FORWARD_TIMEOUT).body(bytes.to_vec());
    for (name, value) in &parts.headers {
        // Hop-by-hop / recomputed headers stay off the forwarded request.
        if matches!(name.as_str(), "host" | "content-length" | "connection" | "transfer-encoding") {
            continue;
        }
        builder = builder.header(name.as_str(), value.as_bytes());
    }
    builder = builder.header(FORWARDED_HEADER, "1");

    let peer_response = match builder.send().await {
        Ok(r) => r,
        Err(err) => {
            tracing::warn!(%err, %owner, "peer forward failed");
            return (
                StatusCode::BAD_GATEWAY,
                Json(ApiError { error: format!("owning server replica unreachable: {err}") }),
            )
                .into_response();
        }
    };

    // The WS moved again between our lookup and the peer handling it. One hop
    // only — surface as a retryable 502 instead of chasing it.
    if peer_response.status() == reqwest::StatusCode::MISDIRECTED_REQUEST {
        return (
            StatusCode::BAD_GATEWAY,
            Json(ApiError { error: "target connection moved during forwarding; retry".into() }),
        )
            .into_response();
    }

    let status =
        StatusCode::from_u16(peer_response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut out = Response::builder().status(status);
    if let Some(headers) = out.headers_mut() {
        for (name, value) in peer_response.headers() {
            if matches!(name.as_str(), "content-length" | "connection" | "transfer-encoding") {
                continue;
            }
            if let (Ok(n), Ok(v)) = (
                axum::http::HeaderName::from_bytes(name.as_ref()),
                HeaderValue::from_bytes(value.as_bytes()),
            ) {
                headers.insert(n, v);
            }
        }
    }
    let body = match peer_response.bytes().await {
        Ok(b) => b,
        Err(err) => {
            tracing::warn!(%err, %owner, "peer response body read failed");
            return (
                StatusCode::BAD_GATEWAY,
                Json(ApiError { error: "peer response truncated".into() }),
            )
                .into_response();
        }
    };
    out.body(axum::body::Body::from(body)).unwrap_or_else(|_| {
        (StatusCode::BAD_GATEWAY, Json(ApiError { error: "peer response invalid".into() }))
            .into_response()
    })
}

#[cfg(test)]
mod tests {
    use super::{NOT_LOCAL_PREFIX, not_local_error, parse_not_local};
    use axum::http::StatusCode;

    #[test]
    fn not_local_roundtrip() {
        let (status, body) = not_local_error("10.0.0.1");
        assert_eq!(status, StatusCode::MISDIRECTED_REQUEST);
        let bytes = serde_json::to_vec(&body.0).unwrap();
        assert_eq!(parse_not_local(&bytes).as_deref(), Some("10.0.0.1"));
    }

    #[test]
    fn parse_rejects_non_ip_owner() {
        // A forged owner must never steer the forwarder at an arbitrary host.
        let forged = serde_json::json!({ "error": format!("{NOT_LOCAL_PREFIX}evil.example.com") });
        assert_eq!(parse_not_local(&serde_json::to_vec(&forged).unwrap()), None);
        let no_prefix = serde_json::json!({ "error": "dispatcher 'x' is offline" });
        assert_eq!(parse_not_local(&serde_json::to_vec(&no_prefix).unwrap()), None);
        assert_eq!(parse_not_local(b"not json"), None);
    }

    #[test]
    fn ws_targeted_paths() {
        use super::is_ws_targeted;
        for p in [
            "/api/v1/sessions/dispatch",
            "/api/v1/sessions/spawn",
            "/api/v1/sessions/abc-123/message",
            "/api/v1/sessions/abc-123/kill",
            "/api/v1/sessions/abc-123/files",
            "/api/v1/sessions/abc-123/launch",
            "/api/v1/machines/uuid-1/fs/dirs",
            // nest-stripped variants must match too
            "/sessions/dispatch",
            "/sessions/abc/fork",
        ] {
            assert!(is_ws_targeted(p), "{p} should be forwardable");
        }
        for p in [
            "/api/v1/sessions",
            "/api/v1/sessions/dispatchers",
            "/api/v1/sessions/abc-123",
            "/api/v1/sessions/abc-123/conversation",
            "/api/v1/sessions/abc-123/labels",
            "/api/v1/archive/proj/sess",
            "/api/v1/skills/name",
            "/api/v1/machines/uuid-1/commands/pending",
        ] {
            assert!(!is_ws_targeted(p), "{p} must pass through unbuffered");
        }
    }

    #[test]
    fn parse_accepts_ipv6() {
        let v6 = serde_json::json!({ "error": format!("{NOT_LOCAL_PREFIX}fd00::1") });
        assert_eq!(parse_not_local(&serde_json::to_vec(&v6).unwrap()).as_deref(), Some("fd00::1"));
    }
}
