//! Serves preview hosts: cookie gate, ticket redemption and the tunnel
//! itself (HTTP streaming both ways, WebSocket passthrough for HMR).

use std::time::Duration;

use axum::body::{Body, HttpBody};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{FromRequestParts, Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use cctui_proto::ws::{DaemonFrameDown, PREVIEW_CHUNK_BYTES, PreviewChunk, PreviewHeader};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;

use super::{Head, Inbound, Preview, Registry, ticket};
use crate::state::AppState;

pub const MAX_REQUEST_BODY: u64 = 25 * 1024 * 1024;
const HEAD_TIMEOUT: Duration = Duration::from_mins(1);

const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// Outermost layer: requests for a preview host never reach the regular
/// router; every other host is untouched.
pub async fn host_gate(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let Some(pattern) = state.preview.host() else {
        return next.run(request).await;
    };
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .or_else(|| request.uri().host().map(str::to_owned));
    match host.and_then(|h| pattern.id_from_host(&h)) {
        Some(id) => handle(state, &id, request).await,
        None => next.run(request).await,
    }
}

pub async fn handle(state: AppState, preview_id: &str, request: Request) -> Response {
    let Some(preview) = state.preview.get(preview_id) else {
        return page(StatusCode::NOT_FOUND, "No such preview", "This preview is not open.");
    };
    if request.uri().path() == "/__cctui/auth" {
        return redeem(&state.preview, &preview, &request);
    }
    let authorized = ticket::cookie_value(request.headers()).is_some_and(|c| {
        state.preview.tickets().check_cookie(&c, &preview.id, preview.user_id).is_ok()
    });
    if !authorized {
        return page(
            StatusCode::UNAUTHORIZED,
            "Preview requires sign-in",
            "Open this preview from your cctui session so it can hand you an access ticket.",
        );
    }
    if is_websocket_upgrade(request.headers()) {
        tunnel_ws(state, preview, request).await
    } else {
        tunnel_http(state, preview, request).await
    }
}

fn redeem(registry: &Registry, preview: &Preview, request: &Request) -> Response {
    let ticket = request
        .uri()
        .query()
        .into_iter()
        .flat_map(|q| q.split('&'))
        .find_map(|pair| pair.strip_prefix("ticket="))
        .unwrap_or("");
    match registry.tickets().redeem_ticket(ticket, &preview.id) {
        Ok(grant) if grant.user_id == preview.user_id => {
            let cookie = registry.tickets().mint_cookie(&grant);
            let https = crate::auth::request_is_https(request.headers());
            (
                StatusCode::FOUND,
                [
                    (header::LOCATION, "/".to_owned()),
                    (header::SET_COOKIE, ticket::set_cookie(&cookie, https)),
                    (header::CACHE_CONTROL, "no-store".to_owned()),
                ],
            )
                .into_response()
        }
        Ok(_) => {
            page(StatusCode::FORBIDDEN, "Not your preview", "This ticket belongs to another user.")
        }
        Err(reject) => page(
            StatusCode::FORBIDDEN,
            "Ticket rejected",
            &format!(
                "The preview ticket was not accepted ({reject:?}). Ask cctui for a fresh one."
            ),
        ),
    }
}

fn page(status: StatusCode, title: &str, text: &str) -> Response {
    let body = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>{title}</title></head>\
         <body style=\"font-family:system-ui;margin:3rem\"><h1>{title}</h1><p>{text}</p></body></html>"
    );
    (
        status,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8"), (header::CACHE_CONTROL, "no-store")],
        body,
    )
        .into_response()
}

fn is_websocket_upgrade(headers: &HeaderMap) -> bool {
    headers
        .get(header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
}

/// Headers the app may see: hop-by-hop, `Host` and cctui's own credentials
/// never cross the tunnel.
fn forwardable_headers(headers: &HeaderMap) -> Vec<PreviewHeader> {
    let mut out = Vec::with_capacity(headers.len());
    for (name, value) in headers {
        let lower = name.as_str();
        if HOP_BY_HOP.contains(&lower) || lower == "host" || lower == "authorization" {
            continue;
        }
        let Ok(value) = value.to_str() else { continue };
        if lower == "cookie" {
            let kept: Vec<&str> = value
                .split(';')
                .map(str::trim)
                .filter(|pair| {
                    let name = pair.split('=').next().unwrap_or("").trim();
                    name != crate::auth::AUTH_COOKIE && name != ticket::COOKIE_NAME
                })
                .collect();
            if !kept.is_empty() {
                out.push(PreviewHeader { name: lower.to_owned(), value: kept.join("; ") });
            }
            continue;
        }
        out.push(PreviewHeader { name: lower.to_owned(), value: value.to_owned() });
    }
    out
}

fn response_from_head(head: &Head) -> axum::http::response::Builder {
    let mut builder = Response::builder().status(head.status);
    for h in &head.headers {
        if HOP_BY_HOP.contains(&h.name.to_ascii_lowercase().as_str()) {
            continue;
        }
        if let (Ok(name), Ok(value)) =
            (HeaderName::try_from(h.name.as_str()), HeaderValue::try_from(h.value.as_str()))
        {
            builder = builder.header(name, value);
        }
    }
    builder
}

fn chunk_frame(stream_id: &str, data: &[u8], text: bool, end: bool) -> DaemonFrameDown {
    DaemonFrameDown::PreviewChunk(PreviewChunk {
        stream_id: stream_id.to_owned(),
        data: BASE64.encode(data),
        text,
        end,
    })
}

async fn send(state: &AppState, preview: &Preview, frame: DaemonFrameDown) -> bool {
    super::send_down(state, preview.machine_id, &preview.session_id, frame).await
}

fn gateway_error(text: &str) -> Response {
    page(StatusCode::BAD_GATEWAY, "Preview unavailable", text)
}

async fn tunnel_http(state: AppState, preview: Preview, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let declared = body.size_hint();
    if declared.lower() > MAX_REQUEST_BODY {
        return page(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Request too large",
            "Preview requests are capped at 25 MB.",
        );
    }
    let has_body = declared.upper() != Some(0);
    let (stream_id, head_rx, body_rx) = state.preview.open_stream(&preview.id);
    let path = parts.uri.path_and_query().map_or("/", |p| p.as_str()).to_owned();
    let open = DaemonFrameDown::PreviewRequest {
        stream_id: stream_id.clone(),
        port: preview.port,
        method: parts.method.as_str().to_owned(),
        path,
        headers: forwardable_headers(&parts.headers),
        upgrade: false,
        has_body,
    };
    if !send(&state, &preview, open).await {
        state.preview.drop_stream(&stream_id);
        return gateway_error("The session's daemon is not connected.");
    }
    if has_body {
        let (state, preview, stream_id) = (state.clone(), preview.clone(), stream_id.clone());
        tokio::spawn(async move {
            if let Err(e) = pump_request_body(&state, &preview, &stream_id, body).await {
                tracing::debug!(%stream_id, "preview request body aborted: {e}");
                super::abort_stream(&state, &state.preview, &preview.id, &stream_id).await;
            }
        });
    }
    let head = match tokio::time::timeout(HEAD_TIMEOUT, head_rx).await {
        Ok(Ok(Ok(head))) => head,
        Ok(Ok(Err(error))) => {
            state.preview.drop_stream(&stream_id);
            return gateway_error(&format!("The dev server did not answer: {error}"));
        }
        Ok(Err(_)) => {
            state.preview.drop_stream(&stream_id);
            return gateway_error("The preview was closed.");
        }
        Err(_) => {
            super::abort_stream(&state, &state.preview, &preview.id, &stream_id).await;
            return page(
                StatusCode::GATEWAY_TIMEOUT,
                "Preview timed out",
                "The dev server took longer than 60 s.",
            );
        }
    };
    let body = Body::from_stream(futures_util::stream::unfold(body_rx, |mut rx| async move {
        match rx.recv().await? {
            Ok(Inbound { data, end, .. }) => {
                if end && data.is_empty() {
                    None
                } else {
                    Some((Ok::<_, std::io::Error>(data), rx))
                }
            }
            Err(error) => Some((Err(std::io::Error::other(error)), rx)),
        }
    }));
    response_from_head(&head)
        .body(body)
        .unwrap_or_else(|_| gateway_error("Malformed upstream response."))
}

async fn pump_request_body(
    state: &AppState,
    preview: &Preview,
    stream_id: &str,
    body: Body,
) -> Result<(), String> {
    let mut stream = body.into_data_stream();
    let mut total: u64 = 0;
    while let Some(piece) = stream.next().await {
        let piece = piece.map_err(|e| e.to_string())?;
        total += piece.len() as u64;
        if total > MAX_REQUEST_BODY {
            return Err("request body over 25 MB".to_owned());
        }
        for part in piece.chunks(PREVIEW_CHUNK_BYTES) {
            if !send(state, preview, chunk_frame(stream_id, part, false, false)).await {
                return Err("daemon link lost".to_owned());
            }
        }
    }
    if send(state, preview, chunk_frame(stream_id, &[], false, true)).await {
        Ok(())
    } else {
        Err("daemon link lost".to_owned())
    }
}

async fn tunnel_ws(state: AppState, preview: Preview, request: Request) -> Response {
    let (mut parts, _) = request.into_parts();
    let (stream_id, head_rx, body_rx) = state.preview.open_stream(&preview.id);
    let path = parts.uri.path_and_query().map_or("/", |p| p.as_str()).to_owned();
    let open = DaemonFrameDown::PreviewRequest {
        stream_id: stream_id.clone(),
        port: preview.port,
        method: "GET".to_owned(),
        path,
        headers: forwardable_headers(&parts.headers),
        upgrade: true,
        has_body: false,
    };
    if !send(&state, &preview, open).await {
        state.preview.drop_stream(&stream_id);
        return gateway_error("The session's daemon is not connected.");
    }
    let head = match tokio::time::timeout(HEAD_TIMEOUT, head_rx).await {
        Ok(Ok(Ok(head))) => head,
        Ok(Ok(Err(error))) => {
            state.preview.drop_stream(&stream_id);
            return gateway_error(&format!("The dev server refused the websocket: {error}"));
        }
        Ok(Err(_)) => {
            state.preview.drop_stream(&stream_id);
            return gateway_error("The preview was closed.");
        }
        Err(_) => {
            super::abort_stream(&state, &state.preview, &preview.id, &stream_id).await;
            return page(
                StatusCode::GATEWAY_TIMEOUT,
                "Preview timed out",
                "The dev server took longer than 60 s.",
            );
        }
    };
    if head.status != 101 {
        super::abort_stream(&state, &state.preview, &preview.id, &stream_id).await;
        return response_from_head(&head)
            .body(Body::empty())
            .unwrap_or_else(|_| gateway_error("Malformed upstream response."));
    }
    let upgrade = match WebSocketUpgrade::from_request_parts(&mut parts, &()).await {
        Ok(upgrade) => upgrade,
        Err(rejection) => {
            super::abort_stream(&state, &state.preview, &preview.id, &stream_id).await;
            return rejection.into_response();
        }
    };
    let protocol = head
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("sec-websocket-protocol"))
        .map(|h| h.value.clone());
    let upgrade = match protocol {
        Some(protocol) => upgrade.protocols([protocol]),
        None => upgrade,
    };
    upgrade.on_upgrade(move |socket| pump_ws(state, preview, stream_id, socket, body_rx))
}

async fn pump_ws(
    state: AppState,
    preview: Preview,
    stream_id: String,
    socket: WebSocket,
    mut body_rx: mpsc::Receiver<Result<Inbound, String>>,
) {
    let (mut sink, mut stream) = socket.split();
    loop {
        tokio::select! {
            msg = stream.next() => {
                let frame = match msg {
                    Some(Ok(Message::Text(text))) => chunk_frame(&stream_id, text.as_bytes(), true, false),
                    Some(Ok(Message::Binary(bytes))) => chunk_frame(&stream_id, &bytes, false, false),
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
                    Some(Ok(Message::Close(_)) | Err(_)) | None => {
                        send(&state, &preview, chunk_frame(&stream_id, &[], false, true)).await;
                        break;
                    }
                };
                if !send(&state, &preview, frame).await {
                    break;
                }
            }
            item = body_rx.recv() => {
                let Some(Ok(Inbound { data, text, end: false })) = item else {
                    let _ = sink.send(Message::Close(None)).await;
                    break;
                };
                    let msg = if text {
                        Message::Text(String::from_utf8_lossy(&data).into_owned().into())
                    } else {
                        Message::Binary(data)
                    };
                    if sink.send(msg).await.is_err() {
                        send(&state, &preview, chunk_frame(&stream_id, &[], false, true)).await;
                        break;
                    }
            }
        }
    }
    state.preview.drop_stream(&stream_id);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forwarded_headers_drop_hop_by_hop_host_and_cctui_credentials() {
        let mut headers = HeaderMap::new();
        headers.insert("host", "cctui-pv-x.example".parse().unwrap());
        headers.insert("connection", "keep-alive".parse().unwrap());
        headers.insert("authorization", "Bearer secret".parse().unwrap());
        headers.insert("cookie", "cctui_auth=secret; app=1; cctui_preview=tok".parse().unwrap());
        headers.insert("accept", "text/html".parse().unwrap());
        let out = forwardable_headers(&headers);
        let names: Vec<&str> = out.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(names, ["cookie", "accept"]);
        assert_eq!(out[0].value, "app=1");

        headers.insert("cookie", "cctui_auth=secret".parse().unwrap());
        assert!(forwardable_headers(&headers).iter().all(|h| h.name != "cookie"));
    }

    #[test]
    fn upstream_head_is_rebuilt_without_hop_by_hop_headers() {
        let head = Head {
            status: 201,
            headers: vec![
                PreviewHeader { name: "Transfer-Encoding".into(), value: "chunked".into() },
                PreviewHeader { name: "content-type".into(), value: "text/plain".into() },
                PreviewHeader { name: "bad header".into(), value: "x".into() },
            ],
        };
        let resp = response_from_head(&head).body(Body::empty()).unwrap();
        assert_eq!(resp.status(), 201);
        assert_eq!(resp.headers().get("content-type").unwrap(), "text/plain");
        assert!(resp.headers().get("transfer-encoding").is_none());
        assert_eq!(resp.headers().len(), 1);
    }
}
