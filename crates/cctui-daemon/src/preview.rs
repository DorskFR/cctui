//! Daemon side of the dev-server preview tunnel.
//!
//! `preview open --port N` registers a loopback port with the server, which
//! then tunnels browser traffic for the preview host down the daemon WS as
//! `PreviewRequest`/`PreviewChunk` frames. Every stream is proxied to
//! `127.0.0.1:N` only; nothing else is reachable through the tunnel.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use cctui_proto::ws::{
    DaemonFrameDown, DaemonFrameUp, PREVIEW_CHUNK_BYTES, PreviewChunk, PreviewHeader,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub const MIN_PORT: u16 = 1024;
pub const MAX_PER_SESSION: usize = 8;
pub const SESSION_ID_VAR: &str = "CCTUI_SESSION_ID";
const OPEN_TIMEOUT: Duration = Duration::from_secs(20);
const REQUEST_TIMEOUT: Duration = Duration::from_mins(1);
const INBOUND_BACKLOG: usize = 32;
const INBOUND_STALL: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opened {
    pub preview_id: String,
    pub url: String,
}

struct StreamHandle {
    inbound: mpsc::Sender<PreviewChunk>,
    cancel: CancellationToken,
}

#[derive(Default)]
struct Registry {
    uplink: Option<mpsc::Sender<DaemonFrameUp>>,
    pending_open: HashMap<Uuid, oneshot::Sender<Result<Opened, String>>>,
    open: HashMap<(String, u16), Opened>,
    streams: HashMap<String, StreamHandle>,
}

static REGISTRY: LazyLock<Mutex<Registry>> = LazyLock::new(|| Mutex::new(Registry::default()));

fn registry() -> std::sync::MutexGuard<'static, Registry> {
    REGISTRY.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Bind (or, with `None`, drop) the live daemon WS uplink.
///
/// Dropping it fails pending opens and aborts in-flight streams, but KEEPS the
/// open previews: the server holds their rows through a grace period, so the
/// reconnect re-announces them by id and the browser's URL survives a rolling
/// restart.
pub fn set_uplink(tx: Option<mpsc::Sender<DaemonFrameUp>>) {
    let reannounce = {
        let mut reg = registry();
        reg.uplink = tx;
        if reg.uplink.is_none() {
            for (_, waiter) in reg.pending_open.drain() {
                let _ = waiter.send(Err("daemon lost its server connection".to_owned()));
            }
            for (_, handle) in reg.streams.drain() {
                handle.cancel.cancel();
            }
            return;
        }
        let uplink = reg.uplink.clone();
        let open: Vec<(String, u16, String)> = reg
            .open
            .iter()
            .map(|((session, port), opened)| (session.clone(), *port, opened.preview_id.clone()))
            .collect();
        drop(reg);
        uplink.zip(Some(open))
    };
    let Some((uplink, open)) = reannounce else { return };
    if open.is_empty() {
        return;
    }
    tokio::spawn(async move {
        for (session_id, port, preview_id) in open {
            let frame = DaemonFrameUp::PreviewOpen {
                request_id: Uuid::new_v4(),
                session_id,
                port,
                preview_id: Some(preview_id),
            };
            if uplink.send(frame).await.is_err() {
                return;
            }
        }
    });
}

/// Forget every open preview: the session is over, not merely disconnected.
pub fn forget_all() {
    registry().open.clear();
}

pub fn validate_port(port: u16) -> Result<(), String> {
    if port < MIN_PORT {
        return Err(format!("port {port} is privileged; previews only tunnel ports >= {MIN_PORT}"));
    }
    Ok(())
}

pub async fn open(session_id: &str, port: u16) -> Result<Opened, String> {
    validate_port(port)?;
    let (tx, rx) = oneshot::channel();
    let request_id = Uuid::new_v4();
    let uplink = {
        let mut reg = registry();
        if let Some(existing) = reg.open.get(&(session_id.to_owned(), port)) {
            return Ok(existing.clone());
        }
        if reg.open.keys().filter(|(s, _)| s == session_id).count() >= MAX_PER_SESSION {
            return Err(format!("session already has {MAX_PER_SESSION} open previews"));
        }
        let uplink = reg.uplink.clone().ok_or("daemon is not connected to the server")?;
        reg.pending_open.insert(request_id, tx);
        uplink
    };
    let frame = DaemonFrameUp::PreviewOpen {
        request_id,
        session_id: session_id.to_owned(),
        port,
        preview_id: None,
    };
    if uplink.send(frame).await.is_err() {
        registry().pending_open.remove(&request_id);
        return Err("daemon is not connected to the server".to_owned());
    }
    let outcome = match tokio::time::timeout(OPEN_TIMEOUT, rx).await {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(_)) => Err("daemon lost its server connection".to_owned()),
        Err(_) => {
            registry().pending_open.remove(&request_id);
            Err("the server did not answer the preview registration in time".to_owned())
        }
    };
    if let Ok(opened) = &outcome {
        registry().open.insert((session_id.to_owned(), port), opened.clone());
    }
    outcome
}

pub async fn close(session_id: &str, port: u16) -> Result<(), String> {
    let uplink = {
        let mut reg = registry();
        reg.open.remove(&(session_id.to_owned(), port));
        reg.uplink.clone().ok_or("daemon is not connected to the server")?
    };
    uplink
        .send(DaemonFrameUp::PreviewClose { session_id: session_id.to_owned(), port })
        .await
        .map_err(|_| "daemon is not connected to the server".to_owned())
}

#[must_use]
pub const fn is_preview_frame(frame: &DaemonFrameDown) -> bool {
    matches!(
        frame,
        DaemonFrameDown::PreviewOpened { .. }
            | DaemonFrameDown::PreviewClosed { .. }
            | DaemonFrameDown::PreviewRequest { .. }
            | DaemonFrameDown::PreviewChunk(_)
            | DaemonFrameDown::PreviewAbort { .. }
    )
}

/// Route one preview frame from the server.
///
/// Chunks are handed to their stream with bounded backpressure; a stream
/// that stalls longer than [`INBOUND_STALL`] is aborted rather than blocking
/// the whole daemon link.
pub async fn handle_down(frame: DaemonFrameDown, up: &mpsc::Sender<DaemonFrameUp>) {
    match frame {
        DaemonFrameDown::PreviewOpened { request_id, ok, preview_id, url, error } => {
            let Some(waiter) = registry().pending_open.remove(&request_id) else { return };
            let outcome = match (ok, preview_id, url) {
                (true, Some(preview_id), Some(url)) => Ok(Opened { preview_id, url }),
                _ => Err(error.unwrap_or_else(|| "server refused the preview".to_owned())),
            };
            let _ = waiter.send(outcome);
        }
        DaemonFrameDown::PreviewClosed { session_id, port } => {
            registry().open.remove(&(session_id, port));
        }
        DaemonFrameDown::PreviewRequest {
            stream_id,
            port,
            method,
            path,
            headers,
            upgrade,
            has_body,
        } => {
            if let Err(error) = validate_port(port) {
                let _ = up.send(DaemonFrameUp::PreviewError { stream_id, error }).await;
                return;
            }
            let (inbound_tx, inbound_rx) = mpsc::channel(INBOUND_BACKLOG);
            let cancel = CancellationToken::new();
            registry().streams.insert(
                stream_id.clone(),
                StreamHandle { inbound: inbound_tx, cancel: cancel.clone() },
            );
            let req = Request { stream_id, port, method, path, headers, has_body };
            let up = up.clone();
            tokio::spawn(async move {
                let id = req.stream_id.clone();
                let run = async {
                    if upgrade {
                        run_ws(req, inbound_rx, &up).await
                    } else {
                        run_http(req, inbound_rx, &up).await
                    }
                };
                let outcome = tokio::select! {
                    () = cancel.cancelled() => Ok(()),
                    outcome = run => outcome,
                };
                if let Err(error) = outcome {
                    let _ =
                        up.send(DaemonFrameUp::PreviewError { stream_id: id.clone(), error }).await;
                }
                registry().streams.remove(&id);
            });
        }
        DaemonFrameDown::PreviewChunk(chunk) => {
            let Some(inbound) = registry().streams.get(&chunk.stream_id).map(|h| h.inbound.clone())
            else {
                return;
            };
            let stream_id = chunk.stream_id.clone();
            if tokio::time::timeout(INBOUND_STALL, inbound.send(chunk)).await.is_err() {
                abort(&stream_id);
            }
        }
        DaemonFrameDown::PreviewAbort { stream_id } => abort(&stream_id),
        _ => {}
    }
}

fn abort(stream_id: &str) {
    let removed = registry().streams.remove(stream_id);
    if let Some(handle) = removed {
        handle.cancel.cancel();
    }
}

struct Request {
    stream_id: String,
    port: u16,
    method: String,
    path: String,
    headers: Vec<PreviewHeader>,
    has_body: bool,
}

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
    "host",
    "content-length",
];

fn forwardable(headers: &[PreviewHeader]) -> impl Iterator<Item = &PreviewHeader> {
    headers.iter().filter(|h| !HOP_BY_HOP.contains(&h.name.to_ascii_lowercase().as_str()))
}

fn local_url(scheme: &str, port: u16, path: &str) -> String {
    let path = if path.starts_with('/') { path } else { "/" };
    format!("{scheme}://127.0.0.1:{port}{path}")
}

fn encode_chunk(stream_id: &str, data: &[u8], text: bool, end: bool) -> DaemonFrameUp {
    DaemonFrameUp::PreviewChunk(PreviewChunk {
        stream_id: stream_id.to_owned(),
        data: BASE64.encode(data),
        text,
        end,
    })
}

fn decode_chunk(chunk: &PreviewChunk) -> Result<Vec<u8>, String> {
    BASE64.decode(&chunk.data).map_err(|e| format!("malformed preview chunk: {e}"))
}

static HTTP: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .expect("preview http client")
});

async fn run_http(
    req: Request,
    inbound: mpsc::Receiver<PreviewChunk>,
    up: &mpsc::Sender<DaemonFrameUp>,
) -> Result<(), String> {
    let method = reqwest::Method::from_bytes(req.method.as_bytes())
        .map_err(|_| format!("unsupported method {}", req.method))?;
    let mut builder = HTTP
        .request(method, local_url("http", req.port, &req.path))
        .timeout(REQUEST_TIMEOUT)
        .header("host", format!("localhost:{}", req.port));
    for h in forwardable(&req.headers) {
        builder = builder.header(h.name.as_str(), h.value.as_str());
    }
    if let Some(len) = req.headers.iter().find(|h| h.name.eq_ignore_ascii_case("content-length")) {
        builder = builder.header("content-length", len.value.as_str());
    }
    if req.has_body {
        let body = futures_util::stream::unfold(inbound, |mut rx| async move {
            let chunk = rx.recv().await?;
            if chunk.end && chunk.data.is_empty() {
                return None;
            }
            Some((decode_chunk(&chunk).map_err(std::io::Error::other), rx))
        });
        builder = builder.body(reqwest::Body::wrap_stream(body));
    }
    let resp = builder.send().await.map_err(|e| format!("upstream request failed: {e}"))?;
    let headers = resp
        .headers()
        .iter()
        .filter(|(name, _)| {
            !HOP_BY_HOP.contains(&name.as_str()) || name.as_str() == "content-length"
        })
        .filter_map(|(name, value)| {
            Some(PreviewHeader {
                name: name.as_str().to_owned(),
                value: value.to_str().ok()?.to_owned(),
            })
        })
        .collect();
    let head = DaemonFrameUp::PreviewResponse {
        stream_id: req.stream_id.clone(),
        status: resp.status().as_u16(),
        headers,
    };
    up.send(head).await.map_err(|_| "daemon link closed".to_owned())?;
    let mut body = resp.bytes_stream();
    while let Some(piece) = body.next().await {
        let piece = piece.map_err(|e| format!("upstream body failed: {e}"))?;
        for part in piece.chunks(PREVIEW_CHUNK_BYTES) {
            up.send(encode_chunk(&req.stream_id, part, false, false))
                .await
                .map_err(|_| "daemon link closed".to_owned())?;
        }
    }
    up.send(encode_chunk(&req.stream_id, &[], false, true))
        .await
        .map_err(|_| "daemon link closed".to_owned())
}

async fn run_ws(
    req: Request,
    mut inbound: mpsc::Receiver<PreviewChunk>,
    up: &mpsc::Sender<DaemonFrameUp>,
) -> Result<(), String> {
    use tokio_tungstenite::tungstenite::Message;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    let mut request = local_url("ws", req.port, &req.path)
        .into_client_request()
        .map_err(|e| format!("bad websocket target: {e}"))?;
    request.headers_mut().insert(
        "host",
        format!("localhost:{}", req.port).parse().map_err(|_| "bad host".to_owned())?,
    );
    // Vite rejects HMR upgrades whose Origin is not its own host.
    request.headers_mut().insert(
        "origin",
        format!("http://localhost:{}", req.port).parse().map_err(|_| "bad origin".to_owned())?,
    );
    for h in forwardable(&req.headers) {
        let lower = h.name.to_ascii_lowercase();
        if (lower == "sec-websocket-protocol" || lower == "cookie")
            && let (Ok(name), Ok(value)) = (
                lower.parse::<tokio_tungstenite::tungstenite::http::HeaderName>(),
                h.value.parse::<tokio_tungstenite::tungstenite::http::HeaderValue>(),
            )
        {
            request.headers_mut().insert(name, value);
        }
    }
    let (ws, response) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(|e| format!("upstream websocket failed: {e}"))?;
    let headers = response
        .headers()
        .iter()
        .filter(|(name, _)| name.as_str() == "sec-websocket-protocol")
        .filter_map(|(name, value)| {
            Some(PreviewHeader {
                name: name.as_str().to_owned(),
                value: value.to_str().ok()?.to_owned(),
            })
        })
        .collect();
    up.send(DaemonFrameUp::PreviewResponse {
        stream_id: req.stream_id.clone(),
        status: 101,
        headers,
    })
    .await
    .map_err(|_| "daemon link closed".to_owned())?;
    let (mut sink, mut stream) = ws.split();
    loop {
        tokio::select! {
            msg = stream.next() => {
                let Some(msg) = msg else { break };
                let msg = msg.map_err(|e| format!("upstream websocket failed: {e}"))?;
                let frame = match msg {
                    Message::Text(text) => encode_chunk(&req.stream_id, text.as_bytes(), true, false),
                    Message::Binary(bytes) => encode_chunk(&req.stream_id, &bytes, false, false),
                    Message::Close(_) => encode_chunk(&req.stream_id, &[], false, true),
                    Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => continue,
                };
                let is_close = matches!(&frame, DaemonFrameUp::PreviewChunk(c) if c.end);
                up.send(frame).await.map_err(|_| "daemon link closed".to_owned())?;
                if is_close {
                    break;
                }
            }
            chunk = inbound.recv() => {
                let Some(chunk) = chunk else {
                    let _ = sink.send(Message::Close(None)).await;
                    break;
                };
                if chunk.end {
                    let _ = sink.send(Message::Close(None)).await;
                    break;
                }
                let data = decode_chunk(&chunk)?;
                let msg = if chunk.text {
                    Message::Text(String::from_utf8_lossy(&data).into_owned().into())
                } else {
                    Message::Binary(data.into())
                };
                sink.send(msg).await.map_err(|e| format!("upstream websocket failed: {e}"))?;
            }
        }
    }
    Ok(())
}

/// `cctui-daemon preview <open|close>`: one call over the local agent-tool
/// socket; the daemon answers with the preview URL (or `ok` on close).
pub fn cli(kind: &str, session: Option<String>, port: u16) -> anyhow::Result<String> {
    let session = session
        .or_else(|| std::env::var(SESSION_ID_VAR).ok())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("--session or ${SESSION_ID_VAR} is required"))?;
    validate_port(port).map_err(|e| anyhow::anyhow!(e))?;
    let request = json!({
        "kind": kind,
        "session_id": session,
        "args": { "port": port },
        "timeout_secs": OPEN_TIMEOUT.as_secs(),
        "proto": 1,
    });
    let sock = crate::agenttool::socket_path();
    let stream = std::os::unix::net::UnixStream::connect(&sock)
        .map_err(|e| anyhow::anyhow!("cannot reach the cctui daemon at {}: {e}", sock.display()))?;
    stream.set_read_timeout(Some(OPEN_TIMEOUT + Duration::from_secs(10)))?;
    let mut writer = &stream;
    writeln!(writer, "{request}")?;
    writer.flush()?;
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line)?;
    let reply: Value =
        serde_json::from_str(line.trim()).map_err(|_| anyhow::anyhow!("malformed daemon reply"))?;
    if reply.get("ok").and_then(Value::as_bool) == Some(true) {
        Ok(reply.get("result").and_then(Value::as_str).unwrap_or("ok").to_owned())
    } else {
        anyhow::bail!(
            "{}",
            reply.get("error").and_then(Value::as_str).unwrap_or("preview call failed")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    async fn recv_up(rx: &mut mpsc::Receiver<DaemonFrameUp>) -> DaemonFrameUp {
        tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("frame").expect("open")
    }

    async fn local_http_server() -> (u16, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { return };
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut tmp = [0u8; 4096];
                    let (head_end, body_len) = loop {
                        let n = sock.read(&mut tmp).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            let head = String::from_utf8_lossy(&buf[..pos]).to_ascii_lowercase();
                            let len = head
                                .lines()
                                .find_map(|l| l.strip_prefix("content-length:"))
                                .and_then(|v| v.trim().parse::<usize>().ok())
                                .unwrap_or(0);
                            break (pos + 4, len);
                        }
                    };
                    while buf.len() < head_end + body_len {
                        let n = sock.read(&mut tmp).await.unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                    }
                    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
                    let body = buf[head_end..].to_vec();
                    let first = head.lines().next().unwrap_or("").to_owned();
                    let host = head
                        .lines()
                        .find(|l| l.to_ascii_lowercase().starts_with("host:"))
                        .unwrap_or("")
                        .to_owned();
                    let echo = format!("{first}|{host}|{}", String::from_utf8_lossy(&body));
                    let resp = format!(
                        "HTTP/1.1 201 Created\r\ncontent-type: text/plain\r\nx-echo: yes\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{echo}",
                        echo.len()
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                });
            }
        });
        (port, task)
    }

    fn request(
        stream_id: &str,
        port: u16,
        method: &str,
        path: &str,
        upgrade: bool,
        has_body: bool,
    ) -> DaemonFrameDown {
        DaemonFrameDown::PreviewRequest {
            stream_id: stream_id.into(),
            port,
            method: method.into(),
            path: path.into(),
            headers: vec![
                PreviewHeader { name: "host".into(), value: "cctui-pv-x.example".into() },
                PreviewHeader { name: "connection".into(), value: "keep-alive".into() },
                PreviewHeader { name: "x-custom".into(), value: "1".into() },
                PreviewHeader { name: "content-length".into(), value: "7".into() },
            ],
            upgrade,
            has_body,
        }
    }

    #[tokio::test]
    async fn http_request_is_tunnelled_to_loopback_with_host_rewritten() {
        let _serial = SERIAL.lock().await;
        let (port, server) = local_http_server().await;
        let (up_tx, mut up_rx) = mpsc::channel(64);
        handle_down(request("s1", port, "POST", "/hello?x=1", false, true), &up_tx).await;
        handle_down(
            DaemonFrameDown::PreviewChunk(PreviewChunk {
                stream_id: "s1".into(),
                data: BASE64.encode(b"payload"),
                text: false,
                end: false,
            }),
            &up_tx,
        )
        .await;
        handle_down(
            DaemonFrameDown::PreviewChunk(PreviewChunk {
                stream_id: "s1".into(),
                data: String::new(),
                text: false,
                end: true,
            }),
            &up_tx,
        )
        .await;
        let DaemonFrameUp::PreviewResponse { stream_id, status, headers } =
            recv_up(&mut up_rx).await
        else {
            panic!("expected response head");
        };
        assert_eq!((stream_id.as_str(), status), ("s1", 201));
        assert!(headers.iter().any(|h| h.name == "x-echo"));
        assert!(!headers.iter().any(|h| h.name == "connection"));
        let mut body = Vec::new();
        loop {
            let DaemonFrameUp::PreviewChunk(chunk) = recv_up(&mut up_rx).await else {
                panic!("chunk")
            };
            body.extend(BASE64.decode(&chunk.data).unwrap());
            if chunk.end {
                break;
            }
        }
        let body = String::from_utf8(body).unwrap();
        assert_eq!(body, format!("POST /hello?x=1 HTTP/1.1|host: localhost:{port}|payload"));
        assert!(registry().streams.is_empty());
        server.abort();
    }

    #[tokio::test]
    async fn privileged_ports_and_dead_upstreams_report_errors() {
        let _serial = SERIAL.lock().await;
        let (up_tx, mut up_rx) = mpsc::channel(8);
        handle_down(request("low", 80, "GET", "/", false, false), &up_tx).await;
        assert!(
            matches!(recv_up(&mut up_rx).await, DaemonFrameUp::PreviewError { ref stream_id, .. } if stream_id == "low")
        );
        let free = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = free.local_addr().unwrap().port();
        drop(free);
        handle_down(request("dead", port, "GET", "/", false, false), &up_tx).await;
        assert!(
            matches!(recv_up(&mut up_rx).await, DaemonFrameUp::PreviewError { ref stream_id, .. } if stream_id == "dead")
        );
    }

    #[tokio::test]
    async fn websocket_upgrade_is_passed_through_both_ways() {
        let _serial = SERIAL.lock().await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let echo = tokio::spawn(async move {
            let (sock, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(sock).await.unwrap();
            while let Some(Ok(msg)) = ws.next().await {
                if msg.is_close() {
                    break;
                }
                ws.send(msg).await.unwrap();
            }
        });
        let (up_tx, mut up_rx) = mpsc::channel(64);
        handle_down(request("w1", port, "GET", "/ws", true, false), &up_tx).await;
        assert!(matches!(
            recv_up(&mut up_rx).await,
            DaemonFrameUp::PreviewResponse { status: 101, .. }
        ));
        handle_down(
            DaemonFrameDown::PreviewChunk(PreviewChunk {
                stream_id: "w1".into(),
                data: BASE64.encode(b"ping!"),
                text: true,
                end: false,
            }),
            &up_tx,
        )
        .await;
        let DaemonFrameUp::PreviewChunk(chunk) = recv_up(&mut up_rx).await else { panic!("chunk") };
        assert!(chunk.text && !chunk.end);
        assert_eq!(BASE64.decode(&chunk.data).unwrap(), b"ping!");
        handle_down(
            DaemonFrameDown::PreviewChunk(PreviewChunk {
                stream_id: "w1".into(),
                data: String::new(),
                text: false,
                end: true,
            }),
            &up_tx,
        )
        .await;
        tokio::time::timeout(Duration::from_secs(5), echo).await.unwrap().unwrap();
    }

    #[tokio::test]
    #[expect(
        clippy::result_large_err,
        reason = "tungstenite's accept_hdr_async callback signature"
    )]
    async fn websocket_upgrade_rewrites_host_and_origin_to_the_loopback_dev_server() {
        let _serial = SERIAL.lock().await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (seen_tx, seen_rx) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (sock, _) = listener.accept().await.unwrap();
            let mut seen_tx = Some(seen_tx);
            let callback = |req: &tokio_tungstenite::tungstenite::handshake::server::Request,
                            resp| {
                let header = |n: &str| {
                    req.headers().get(n).and_then(|v| v.to_str().ok()).unwrap_or("").to_owned()
                };
                let seen = (header("host"), header("origin"), header("cookie"));
                let _ = seen_tx.take().unwrap().send(seen);
                Ok::<_, tokio_tungstenite::tungstenite::handshake::server::ErrorResponse>(resp)
            };
            let _ws = tokio_tungstenite::accept_hdr_async(sock, callback).await.unwrap();
        });

        let frame = DaemonFrameDown::PreviewRequest {
            stream_id: "w2".into(),
            port,
            method: "GET".into(),
            path: "/ws".into(),
            headers: vec![
                PreviewHeader { name: "host".into(), value: "cctui-pv-x.example".into() },
                PreviewHeader { name: "origin".into(), value: "https://cctui-pv-x.example".into() },
                PreviewHeader { name: "cookie".into(), value: "app=1".into() },
            ],
            upgrade: true,
            has_body: false,
        };
        let (up_tx, mut up_rx) = mpsc::channel(64);
        handle_down(frame, &up_tx).await;
        assert!(matches!(
            recv_up(&mut up_rx).await,
            DaemonFrameUp::PreviewResponse { status: 101, .. }
        ));

        let (host, origin, cookie) =
            tokio::time::timeout(Duration::from_secs(5), seen_rx).await.unwrap().unwrap();
        assert_eq!(host, format!("localhost:{port}"), "Host follows the loopback target");
        assert_eq!(origin, format!("http://localhost:{port}"), "Origin follows the rewritten Host");
        assert_eq!(cookie, "app=1", "the app's own cookies still reach it");
        server.abort();
    }

    #[tokio::test]
    async fn open_requires_a_link_and_resolves_from_preview_opened() {
        let _serial = SERIAL.lock().await;
        set_uplink(None);
        assert!(open("sess", 5173).await.is_err());
        assert!(open("sess", 80).await.unwrap_err().contains("privileged"));
        let (up_tx, mut up_rx) = mpsc::channel(8);
        set_uplink(Some(up_tx.clone()));
        let opener = tokio::spawn(open("sess", 5173));
        let DaemonFrameUp::PreviewOpen { request_id, session_id, port, preview_id } =
            recv_up(&mut up_rx).await
        else {
            panic!("expected PreviewOpen");
        };
        assert_eq!((session_id.as_str(), port), ("sess", 5173));
        assert!(preview_id.is_none(), "a fresh open asks the server for a new id");
        handle_down(
            DaemonFrameDown::PreviewOpened {
                request_id,
                ok: true,
                preview_id: Some("abc".into()),
                url: Some("https://cctui-pv-abc.example".into()),
                error: None,
            },
            &up_tx,
        )
        .await;
        let link = opener.await.unwrap().unwrap();
        assert_eq!(link.url, "https://cctui-pv-abc.example");
        assert_eq!(open("sess", 5173).await.unwrap(), link);
        close("sess", 5173).await.unwrap();
        assert!(matches!(
            recv_up(&mut up_rx).await,
            DaemonFrameUp::PreviewClose { port: 5173, .. }
        ));
        assert!(registry().open.is_empty());
        set_uplink(None);
    }

    #[tokio::test]
    async fn a_reconnect_re_announces_open_previews_with_their_existing_ids() {
        let _serial = SERIAL.lock().await;
        set_uplink(None);
        forget_all();

        let (up_tx, mut up_rx) = mpsc::channel(8);
        set_uplink(Some(up_tx.clone()));
        let opener = tokio::spawn(open("sess", 5173));
        let DaemonFrameUp::PreviewOpen { request_id, .. } = recv_up(&mut up_rx).await else {
            panic!("expected PreviewOpen");
        };
        handle_down(
            DaemonFrameDown::PreviewOpened {
                request_id,
                ok: true,
                preview_id: Some("keepme".into()),
                url: Some("https://cctui-pv-keepme.example".into()),
                error: None,
            },
            &up_tx,
        )
        .await;
        opener.await.unwrap().unwrap();

        set_uplink(None);
        assert!(!registry().open.is_empty(), "a disconnect keeps the preview for the reconnect");

        let (up2_tx, mut up2_rx) = mpsc::channel(8);
        set_uplink(Some(up2_tx));
        let DaemonFrameUp::PreviewOpen { session_id, port, preview_id, .. } =
            recv_up(&mut up2_rx).await
        else {
            panic!("expected a re-announced PreviewOpen");
        };
        assert_eq!((session_id.as_str(), port), ("sess", 5173));
        assert_eq!(preview_id.as_deref(), Some("keepme"), "the id is re-announced, not reminted");

        set_uplink(None);
        forget_all();
    }

    #[test]
    fn cli_refuses_privileged_ports_before_touching_the_socket() {
        let err = cli("preview_open", Some("sess".into()), 80).unwrap_err().to_string();
        assert!(err.contains("privileged"));
    }
}
