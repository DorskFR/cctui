//! Persistent connection to the shared `codex app-server` daemon.
//!
//! The control socket at `$CODEX_HOME/app-server-control/app-server-control.sock`
//! speaks **WebSocket, not newline-delimited JSON**: a bare `connect()` +
//! `write(json)` is dropped by the server with `failed to upgrade control
//! socket websocket connection`. This is undocumented upstream and is the one
//! thing that makes this module more than a socket swap — after the HTTP/1.1
//! `Upgrade` the JSON-RPC is byte-identical to what [`super::app_server`]
//! writes over stdio.
//!
//! Reads (`thread/list`, `thread/read`, `thread/turns/list`) and the
//! `thread/{archive,unarchive}` lifecycle ops all answer unauthenticated,
//! which is what lets one connection serve every session's inventory
//! regardless of which account owns the thread.
//!
//! Responses and notifications interleave on the one socket, so requests are
//! correlated by JSON-RPC id and everything else fans out to
//! [`DaemonHandle::subscribe`].

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::UnixStream;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

const RPC_TIMEOUT: Duration = Duration::from_secs(30);
const BACKOFF_MIN: Duration = Duration::from_millis(250);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// Sized so a slow consumer lags (and learns of it via
/// [`broadcast::error::RecvError::Lagged`]) rather than stalling the read loop
/// for every other consumer.
const NOTIFY_BUFFER: usize = 1024;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn next_id() -> i64 {
    i64::try_from(NEXT_ID.fetch_add(1, Ordering::Relaxed)).unwrap_or(i64::MAX)
}

/// Where a shared app-server can be reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonEndpoint {
    pub socket: PathBuf,
}

impl DaemonEndpoint {
    /// `codex app-server daemon start` is idempotent and prints its state as
    /// JSON, so it doubles as discovery: it yields the socket path *and*
    /// guarantees the daemon is up. Preferred over deriving the path from
    /// `CODEX_HOME`, which cannot do the latter.
    pub async fn discover(bin: &str) -> Result<Self> {
        if let Some(path) = std::env::var_os("CCTUI_CODEX_APP_SERVER_SOCK") {
            return Ok(Self { socket: PathBuf::from(path) });
        }
        let mut cmd = tokio::process::Command::new(bin);
        cmd.arg("app-server")
            .arg("daemon")
            .arg("start")
            .env("PATH", crate::childenv::child_path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        crate::childenv::ScrubChildEnv::scrub_child_env(&mut cmd);
        let out = tokio::time::timeout(RPC_TIMEOUT, cmd.output())
            .await
            .map_err(|_| anyhow::anyhow!("codex app-server daemon start timed out"))??;
        anyhow::ensure!(out.status.success(), "codex app-server daemon start failed");
        parse_daemon_start(&String::from_utf8_lossy(&out.stdout))
    }
}

/// Scan for the first line that parses as JSON and carries a `socketPath`, so
/// a leading warning line does not defeat discovery.
pub fn parse_daemon_start(stdout: &str) -> Result<DaemonEndpoint> {
    for line in stdout.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else { continue };
        if let Some(path) = v.get("socketPath").and_then(Value::as_str).filter(|s| !s.is_empty()) {
            return Ok(DaemonEndpoint { socket: PathBuf::from(path) });
        }
    }
    anyhow::bail!("codex app-server daemon start reported no socketPath")
}

#[derive(Debug, Clone)]
pub enum DaemonEvent {
    Notification {
        method: String,
        params: Value,
    },
    /// Generation increments on every successful connect. Consumers holding
    /// derived state must resynchronize from a full snapshot: notifications
    /// emitted while the socket was down were delivered to nobody, so only a
    /// fresh read can close the gap.
    Connected {
        generation: u64,
    },
    Disconnected {
        generation: u64,
    },
}

enum Op {
    Request { method: String, params: Value, reply: oneshot::Sender<Result<Value>> },
}

/// Cloneable handle; every clone talks to the same socket.
#[derive(Clone)]
pub struct DaemonHandle {
    ops: mpsc::Sender<Op>,
    events: broadcast::Sender<DaemonEvent>,
}

impl DaemonHandle {
    /// Fails rather than blocking when the connection is down, so callers can
    /// fall back to their own transport.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value> {
        let (tx, rx) = oneshot::channel();
        self.ops
            .send(Op::Request { method: method.to_owned(), params, reply: tx })
            .await
            .map_err(|_| anyhow::anyhow!("codex daemon connection closed"))?;
        rx.await.map_err(|_| anyhow::anyhow!("codex daemon dropped request `{method}`"))?
    }

    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<DaemonEvent> {
        self.events.subscribe()
    }
}

/// Spawn the supervisor and hand back a handle. The handle is usable
/// immediately: requests issued before the first connect fail, and callers
/// fall back.
#[must_use]
pub fn connect(endpoint: DaemonEndpoint, shutdown: CancellationToken) -> DaemonHandle {
    let (ops_tx, ops_rx) = mpsc::channel(256);
    let (events_tx, _) = broadcast::channel(NOTIFY_BUFFER);
    let handle = DaemonHandle { ops: ops_tx, events: events_tx.clone() };
    tokio::spawn(supervise(endpoint, ops_rx, events_tx, shutdown));
    handle
}

async fn supervise(
    endpoint: DaemonEndpoint,
    mut ops: mpsc::Receiver<Op>,
    events: broadcast::Sender<DaemonEvent>,
    shutdown: CancellationToken,
) {
    let mut backoff = BACKOFF_MIN;
    let mut generation = 0_u64;
    loop {
        if shutdown.is_cancelled() {
            return;
        }
        match handshake(&endpoint).await {
            Ok(stream) => {
                generation += 1;
                backoff = BACKOFF_MIN;
                let _ = events.send(DaemonEvent::Connected { generation });
                tracing::info!(
                    socket = %endpoint.socket.display(),
                    generation,
                    "codex: shared app-server connection established"
                );
                pump(stream, &mut ops, &events, &shutdown).await;
                let _ = events.send(DaemonEvent::Disconnected { generation });
                if shutdown.is_cancelled() {
                    return;
                }
                tracing::warn!(generation, "codex: shared app-server connection dropped");
            }
            Err(err) => tracing::debug!(%err, "codex: shared app-server connect failed"),
        }
        tokio::select! {
            () = shutdown.cancelled() => return,
            () = tokio::time::sleep(backoff) => {}
        }
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

type WsStream = tokio_tungstenite::WebSocketStream<UnixStream>;

async fn handshake(endpoint: &DaemonEndpoint) -> Result<WsStream> {
    let stream = UnixStream::connect(&endpoint.socket)
        .await
        .with_context(|| format!("connect {}", endpoint.socket.display()))?;
    // Meaningless over a unix socket, but the WebSocket client requires a URI;
    // the daemon accepts the upgrade on any path.
    let (mut ws, _) = tokio_tungstenite::client_async("ws://localhost/", stream)
        .await
        .context("codex control socket websocket upgrade")?;

    let id = next_id();
    ws.send(Message::Text(initialize_req(id).to_string().into())).await?;
    let resp = tokio::time::timeout(RPC_TIMEOUT, read_response(&mut ws, id))
        .await
        .map_err(|_| anyhow::anyhow!("codex daemon initialize timed out"))??;
    super::app_server::record_codex_version(&resp);
    ws.send(Message::Text(super::app_server::initialized_notification().to_string().into()))
        .await?;
    Ok(ws)
}

fn initialize_req(id: i64) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "initialize",
        "params": {"clientInfo": {"name": "cctui", "version": env!("CARGO_PKG_VERSION")}},
    })
}

/// Returns the whole JSON-RPC envelope, which is what
/// [`super::app_server::record_codex_version`] expects.
async fn read_response(ws: &mut WsStream, id: i64) -> Result<Value> {
    while let Some(frame) = ws.next().await {
        let Message::Text(text) = frame? else { continue };
        let Ok(v) = serde_json::from_str::<Value>(&text) else { continue };
        if v.get("id").and_then(Value::as_i64) == Some(id) {
            if let Some(err) = v.get("error") {
                anyhow::bail!("codex daemon initialize error: {err}");
            }
            return Ok(v);
        }
    }
    anyhow::bail!("codex daemon closed during initialize")
}

/// Drive one live connection until it drops. `pending` lives here, so a drop
/// necessarily fails every in-flight request before `supervise` reconnects —
/// nothing survives a reconnect as a silently-pending request.
async fn pump(
    mut ws: WsStream,
    ops: &mut mpsc::Receiver<Op>,
    events: &broadcast::Sender<DaemonEvent>,
    shutdown: &CancellationToken,
) {
    let mut pending: HashMap<i64, oneshot::Sender<Result<Value>>> = HashMap::new();
    loop {
        tokio::select! {
            () = shutdown.cancelled() => break,
            op = ops.recv() => {
                let Some(Op::Request { method, params, reply }) = op else { break };
                let id = next_id();
                let frame = json!({
                    "jsonrpc": "2.0", "id": id, "method": method, "params": params,
                });
                if let Err(err) = ws.send(Message::Text(frame.to_string().into())).await {
                    let _ = reply.send(Err(anyhow::anyhow!("codex daemon write failed: {err}")));
                    break;
                }
                pending.insert(id, reply);
            }
            frame = ws.next() => {
                match frame {
                    Some(Ok(Message::Text(text))) => dispatch(&text, &mut pending, events),
                    Some(Ok(Message::Ping(payload))) => {
                        if ws.send(Message::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(_)) => {}
                    Some(Err(_)) | None => break,
                }
            }
        }
    }
    for (id, reply) in pending {
        let _ = reply.send(Err(anyhow::anyhow!(
            "codex daemon connection dropped before request {id} was answered"
        )));
    }
}

fn dispatch(
    text: &str,
    pending: &mut HashMap<i64, oneshot::Sender<Result<Value>>>,
    events: &broadcast::Sender<DaemonEvent>,
) {
    let Ok(v) = serde_json::from_str::<Value>(text) else { return };
    if let Some(id) = v.get("id").and_then(Value::as_i64)
        && let Some(reply) = pending.remove(&id)
    {
        let outcome = v.get("error").map_or_else(
            || Ok(v.get("result").cloned().unwrap_or(Value::Null)),
            |err| Err(anyhow::anyhow!("codex daemon error: {err}")),
        );
        let _ = reply.send(outcome);
        return;
    }
    if let Some(method) = v.get("method").and_then(Value::as_str) {
        let _ = events.send(DaemonEvent::Notification {
            method: method.to_owned(),
            params: v.get("params").cloned().unwrap_or(Value::Null),
        });
    }
}

/// Lazily-built shared connection. A discovery failure is cached as "no daemon
/// here" so callers fall back to stdio without re-probing on every RPC.
#[derive(Clone)]
pub struct SharedDaemon {
    inner: Arc<tokio::sync::OnceCell<Option<DaemonHandle>>>,
    bin: String,
    shutdown: CancellationToken,
}

impl SharedDaemon {
    #[must_use]
    pub fn new(bin: String, shutdown: CancellationToken) -> Self {
        Self { inner: Arc::new(tokio::sync::OnceCell::new()), bin, shutdown }
    }

    /// Bypass discovery for a known endpoint.
    #[must_use]
    pub fn from_endpoint(endpoint: DaemonEndpoint, shutdown: CancellationToken) -> Self {
        let cell = tokio::sync::OnceCell::new();
        let _ = cell.set(Some(connect(endpoint, shutdown.clone())));
        Self { inner: Arc::new(cell), bin: String::new(), shutdown }
    }

    pub async fn handle(&self) -> Option<DaemonHandle> {
        self.inner
            .get_or_init(|| async {
                match DaemonEndpoint::discover(&self.bin).await {
                    Ok(endpoint) => Some(connect(endpoint, self.shutdown.clone())),
                    Err(err) => {
                        tracing::info!(
                            %err,
                            "codex: no shared app-server daemon; falling back to per-op stdio"
                        );
                        None
                    }
                }
            })
            .await
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_start_report_yields_the_socket_path() {
        let out = r#"{"status":"started","backend":"pid","pid":1,"socketPath":"/tmp/a.sock"}"#;
        assert_eq!(parse_daemon_start(out).unwrap().socket, PathBuf::from("/tmp/a.sock"));
    }

    #[test]
    fn daemon_start_report_tolerates_leading_log_lines() {
        let out = "warning: experimental\n{\"socketPath\":\"/x/y.sock\"}\n";
        assert_eq!(parse_daemon_start(out).unwrap().socket, PathBuf::from("/x/y.sock"));
    }

    #[test]
    fn daemon_start_without_socket_path_is_an_error() {
        assert!(parse_daemon_start("{\"status\":\"started\"}").is_err());
        assert!(parse_daemon_start("not json").is_err());
    }

    #[test]
    fn rpc_ids_are_unique() {
        assert_ne!(next_id(), next_id());
    }

    #[tokio::test]
    async fn dispatch_completes_a_pending_request() {
        let (tx, rx) = oneshot::channel();
        let mut pending = HashMap::from([(7, tx)]);
        let (events, _guard) = broadcast::channel(8);
        dispatch(r#"{"id":7,"result":{"ok":true}}"#, &mut pending, &events);
        assert!(pending.is_empty());
        assert_eq!(rx.await.unwrap().unwrap(), json!({"ok": true}));
    }

    #[tokio::test]
    async fn dispatch_surfaces_a_jsonrpc_error() {
        let (tx, rx) = oneshot::channel();
        let mut pending = HashMap::from([(7, tx)]);
        let (events, _guard) = broadcast::channel(8);
        dispatch(r#"{"id":7,"error":{"code":-32601,"message":"nope"}}"#, &mut pending, &events);
        let err = rx.await.unwrap().unwrap_err().to_string();
        assert!(err.contains("nope"), "{err}");
    }

    #[test]
    fn dispatch_fans_out_notifications() {
        let mut pending = HashMap::new();
        let (events, mut rx) = broadcast::channel(8);
        dispatch(
            r#"{"method":"thread/started","params":{"threadId":"t1"}}"#,
            &mut pending,
            &events,
        );
        match rx.try_recv().unwrap() {
            DaemonEvent::Notification { method, params } => {
                assert_eq!(method, "thread/started");
                assert_eq!(params["threadId"], "t1");
            }
            other => panic!("expected a notification, got {other:?}"),
        }
    }

    /// A response nobody awaits has no `method` and must not be mistaken for a
    /// notification.
    #[test]
    fn dispatch_ignores_an_unmatched_response() {
        let mut pending = HashMap::new();
        let (events, mut rx) = broadcast::channel(8);
        dispatch(r#"{"id":99,"result":{}}"#, &mut pending, &events);
        assert!(rx.try_recv().is_err());
    }
}

/// A minimal app-server stand-in: answers `initialize` and whatever canned
/// responses the case needs, so the transport can be exercised without a
/// real codex.
#[cfg(test)]
pub(super) mod testserver {
    use super::*;
    use tokio::net::UnixListener;

    /// Serves one connection, replying to every request with
    /// `responses(method) -> result`.
    pub fn spawn<F>(path: &std::path::Path, responses: F) -> tokio::task::JoinHandle<()>
    where
        F: Fn(&str, &Value) -> Value + Send + 'static,
    {
        let listener = UnixListener::bind(path).expect("bind test socket");
        tokio::spawn(async move {
            let Ok((stream, _)) = listener.accept().await else { return };
            let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await else { return };
            while let Some(Ok(frame)) = ws.next().await {
                let Message::Text(text) = frame else { continue };
                let Ok(v) = serde_json::from_str::<Value>(&text) else { continue };
                let Some(id) = v.get("id").and_then(Value::as_i64) else { continue };
                let method = v.get("method").and_then(Value::as_str).unwrap_or("");
                let params = v.get("params").cloned().unwrap_or(Value::Null);
                let result = if method == "initialize" {
                    json!({"userAgent": "codex/0.153.4"})
                } else {
                    responses(method, &params)
                };
                let reply = json!({"jsonrpc": "2.0", "id": id, "result": result});
                if ws.send(Message::Text(reply.to_string().into())).await.is_err() {
                    return;
                }
            }
        })
    }
}
