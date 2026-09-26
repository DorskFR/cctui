//! Pod-to-pod leg of a preview request. The daemon's WS lives on one replica,
//! but the browser can land on any of them, so a pod that does not hold the
//! link reverse-proxies the raw request to the one that does.
//!
//! The browser's cookie is verified *before* this hop and never travels on it:
//! the forwarded request carries only the already-verified user id, under the
//! cluster-internal secret. The receiving pod therefore serves locally and
//! never forwards again, so a stale presence row cannot start a loop.

use axum::body::Body;
use axum::extract::FromRequestParts;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Request, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use uuid::Uuid;

use crate::state::AppState;

/// Carries the verified owner across the internal hop.
pub const USER_HEADER: &str = "x-cctui-preview-user";
/// Set by the forwarding pod so the receiver refuses to forward again.
pub const HOP_HEADER: &str = "x-cctui-preview-hop";

pub fn internal_url(peer_ip: &str, port: u16, preview_id: &str, path_and_query: &str) -> String {
    let host = if peer_ip.contains(':') { format!("[{peer_ip}]") } else { peer_ip.to_owned() };
    let path = path_and_query.strip_prefix('/').unwrap_or(path_and_query);
    format!("http://{host}:{port}/internal/preview/{preview_id}/{path}")
}

/// Headers that must not cross the internal hop: the browser's cctui
/// credentials, and anything hop-by-hop for the pod-to-pod connection itself.
fn skip_on_hop(name: &str) -> bool {
    matches!(
        name,
        "host"
            | "cookie"
            | "authorization"
            | "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "content-length"
    ) || name.starts_with("sec-websocket-")
}

fn hop_headers(src: &HeaderMap, user: Uuid) -> HeaderMap {
    let mut out = HeaderMap::new();
    for (name, value) in src {
        if !skip_on_hop(name.as_str()) {
            out.insert(name.clone(), value.clone());
        }
    }
    if let Ok(value) = HeaderValue::from_str(&user.to_string())
        && let Ok(name) = HeaderName::from_bytes(USER_HEADER.as_bytes())
    {
        out.insert(name, value);
    }
    if let Ok(name) = HeaderName::from_bytes(HOP_HEADER.as_bytes()) {
        out.insert(name, HeaderValue::from_static("1"));
    }
    out
}

fn bad_gateway(text: &str) -> Response {
    (StatusCode::BAD_GATEWAY, text.to_owned()).into_response()
}

/// Reverse-proxy a plain HTTP preview request to the pod holding the link,
/// streaming the request and response bodies rather than buffering them.
pub async fn http(
    state: &AppState,
    peer_ip: &str,
    preview_id: &str,
    user: Uuid,
    request: Request<Body>,
) -> Response {
    let Some(secret) = state.internal_secret.clone() else {
        return bad_gateway("This replica cannot reach the preview's pod.");
    };
    let (parts, body) = request.into_parts();
    let path = parts.uri.path_and_query().map_or("/", |p| p.as_str());
    let url = internal_url(peer_ip, state.config.port, preview_id, path);

    let upstream = state
        .http_client
        .request(parts.method.clone(), &url)
        .bearer_auth(&secret)
        .headers(hop_headers(&parts.headers, user))
        .body(reqwest::Body::wrap_stream(body.into_data_stream()))
        .send()
        .await;
    let upstream = match upstream {
        Ok(r) => r,
        Err(err) => {
            tracing::warn!(%err, %peer_ip, %preview_id, "preview peer forward failed");
            return bad_gateway("The preview's pod did not answer.");
        }
    };

    let mut response = Response::builder().status(upstream.status());
    for (name, value) in upstream.headers() {
        if !is_hop_by_hop_response(name.as_str()) {
            response = response.header(name, value);
        }
    }
    response
        .body(Body::from_stream(upstream.bytes_stream()))
        .unwrap_or_else(|_| bad_gateway("Malformed response from the preview's pod."))
}

fn is_hop_by_hop_response(name: &str) -> bool {
    matches!(
        name,
        "connection"
            | "keep-alive"
            | "transfer-encoding"
            | "trailer"
            | "upgrade"
            | "content-length"
    )
}

/// Bridge a browser WebSocket to the pod holding the link: accept the browser's
/// upgrade only once the peer's own upgrade succeeded, then copy messages both
/// ways until either side closes.
pub async fn websocket(
    state: &AppState,
    peer_ip: &str,
    preview_id: &str,
    user: Uuid,
    request: Request<Body>,
) -> Response {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    let Some(secret) = state.internal_secret.clone() else {
        return bad_gateway("This replica cannot reach the preview's pod.");
    };
    let (mut parts, _) = request.into_parts();
    let path = parts.uri.path_and_query().map_or("/", |p| p.as_str());
    let url = internal_url(peer_ip, state.config.port, preview_id, path);
    let ws_url = url.replacen("http://", "ws://", 1);

    let mut peer_request = match ws_url.as_str().into_client_request() {
        Ok(r) => r,
        Err(err) => {
            tracing::warn!(%err, %preview_id, "preview peer ws url rejected");
            return bad_gateway("The preview's pod could not be addressed.");
        }
    };
    {
        let headers = peer_request.headers_mut();
        for (name, value) in &hop_headers(&parts.headers, user) {
            headers.insert(name.clone(), value.clone());
        }
        if let Ok(value) = HeaderValue::from_str(&format!("Bearer {secret}")) {
            headers.insert(header::AUTHORIZATION, value);
        }
    }

    let (peer_ws, _) = match tokio_tungstenite::connect_async(peer_request).await {
        Ok(ok) => ok,
        Err(err) => {
            tracing::warn!(%err, %peer_ip, %preview_id, "preview peer ws forward failed");
            return bad_gateway("The preview's pod refused the websocket.");
        }
    };

    let upgrade = match WebSocketUpgrade::from_request_parts(&mut parts, &()).await {
        Ok(upgrade) => upgrade,
        Err(rejection) => return rejection.into_response(),
    };
    upgrade.on_upgrade(move |browser| pump(browser, peer_ws))
}

type PeerWs =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn pump(browser: WebSocket, peer: PeerWs) {
    use tokio_tungstenite::tungstenite::Message as Peer;

    let (mut browser_tx, mut browser_rx) = browser.split();
    let (mut peer_tx, mut peer_rx) = peer.split();

    let to_peer = async {
        while let Some(Ok(msg)) = browser_rx.next().await {
            let out = match msg {
                Message::Text(t) => Peer::Text(t.as_str().into()),
                Message::Binary(b) => Peer::Binary(b),
                Message::Ping(p) => Peer::Ping(p),
                Message::Pong(p) => Peer::Pong(p),
                Message::Close(_) => break,
            };
            if peer_tx.send(out).await.is_err() {
                break;
            }
        }
        let _ = peer_tx.close().await;
    };

    let to_browser = async {
        while let Some(Ok(msg)) = peer_rx.next().await {
            let out = match msg {
                Peer::Text(t) => Message::Text(t.as_str().into()),
                Peer::Binary(b) => Message::Binary(b),
                Peer::Ping(p) => Message::Ping(p),
                Peer::Pong(p) => Message::Pong(p),
                Peer::Close(_) => break,
                Peer::Frame(_) => continue,
            };
            if browser_tx.send(out).await.is_err() {
                break;
            }
        }
        let _ = browser_tx.close().await;
    };

    tokio::join!(to_peer, to_browser);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_urls_are_built_per_peer_and_path() {
        assert_eq!(
            internal_url("10.0.0.7", 8700, "abc", "/index.html?x=1"),
            "http://10.0.0.7:8700/internal/preview/abc/index.html?x=1"
        );
        assert_eq!(
            internal_url("10.0.0.7", 8700, "abc", "/"),
            "http://10.0.0.7:8700/internal/preview/abc/"
        );
        assert!(
            internal_url("fd00::1", 8700, "abc", "/a").starts_with("http://[fd00::1]:8700/"),
            "ipv6 peers are bracketed"
        );
    }

    #[test]
    fn the_hop_drops_browser_credentials_and_pins_the_user() {
        let user = Uuid::new_v4();
        let mut src = HeaderMap::new();
        src.insert(header::COOKIE, HeaderValue::from_static("cctui_session=secret"));
        src.insert(header::AUTHORIZATION, HeaderValue::from_static("Bearer user-token"));
        src.insert(header::HOST, HeaderValue::from_static("cctui-pv-abc.example"));
        src.insert(header::ACCEPT, HeaderValue::from_static("text/html"));
        src.insert("sec-websocket-key", HeaderValue::from_static("abc"));

        let out = hop_headers(&src, user);
        assert!(out.get(header::COOKIE).is_none(), "the browser cookie never crosses the hop");
        assert!(out.get(header::AUTHORIZATION).is_none());
        assert!(out.get(header::HOST).is_none());
        assert!(out.get("sec-websocket-key").is_none());
        assert_eq!(out.get(header::ACCEPT).unwrap(), "text/html");
        assert_eq!(out.get(USER_HEADER).unwrap(), &user.to_string());
        assert_eq!(out.get(HOP_HEADER).unwrap(), "1");
    }
}
