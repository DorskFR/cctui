//! The one module that touches the ACP SDK.
//!
//! The SDK owns JSON-RPC framing, ids and cancellation over the agent's
//! stdio; everything it hands us is forwarded as JSON on an unbounded inbox,
//! and nothing waits inside a receive callback. Requests go out untyped
//! (`protocol.rs` builds them) so the adapter reads the whole answer.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use agent_client_protocol::{Agent, ByteStreams, Client, ConnectionTo, Responder, UntypedMessage};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{mpsc, oneshot};
use tokio_util::compat::{TokioAsyncReadCompatExt as _, TokioAsyncWriteCompatExt as _};

use super::process::{Launch, Spawned};
use crate::adapters::traffic_rings::TrafficRings;

/// A line the tap buffers before giving up on finding its newline.
const TAP_LINE_MAX: usize = 1 << 20;

/// What the agent sent us.
#[derive(Debug)]
pub enum Incoming {
    /// A notification (`session/update` and friends), params raw.
    Notification { method: String, params: Value },
    /// `session/request_permission`; answer by sending the response JSON on
    /// `reply`, or drop it to answer `cancelled`.
    Permission { params: Value, reply: oneshot::Sender<Value> },
    /// The connection ended: agent EOF, process exit or a transport failure.
    Closed { detail: Option<String> },
}

/// A JSON-RPC failure the agent answered, kept apart from transport errors
/// so `-32000` (not logged in) can be worded for the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (code {})", self.message, self.code)
    }
}

impl std::error::Error for RpcError {}

/// The request side of a connection, usable off the session task.
#[derive(Clone)]
pub struct Requester {
    cx: ConnectionTo<Agent>,
}

impl Requester {
    /// One JSON-RPC request. A timeout is an `Err` like any other transport
    /// failure; an error the agent answered is an [`RpcError`].
    pub async fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> anyhow::Result<Value> {
        let message = UntypedMessage::new(method, params)
            .map_err(|e| anyhow::anyhow!("encoding {method}: {}", rpc_error_text(&e)))?;
        let sent = self.cx.send_request(message);
        match tokio::time::timeout(timeout, sent.block_task()).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(err)) => Err(RpcError {
                code: i64::from(i32::from(err.code)),
                message: rpc_error_text(&err),
            }
            .into()),
            Err(_) => Err(anyhow::anyhow!("{method} did not answer within {timeout:?}")),
        }
    }

    pub fn notify(&self, method: &str, params: Value) -> anyhow::Result<()> {
        let message = UntypedMessage::new(method, params)
            .map_err(|e| anyhow::anyhow!("encoding {method}: {}", rpc_error_text(&e)))?;
        self.cx
            .send_notification(message)
            .map_err(|e| anyhow::anyhow!("sending {method}: {}", rpc_error_text(&e)))
    }
}

/// A live connection to one agent process.
pub struct AcpConnection {
    requester: Requester,
    close: Option<oneshot::Sender<()>>,
    pub child: tokio::process::Child,
}

impl AcpConnection {
    /// Spawn the agent and run the protocol on its own task. Returns once the
    /// connection can take requests; a transport that fails first is the error.
    pub async fn open(
        launch: &Launch,
        rings: Arc<TrafficRings>,
        inbox: mpsc::UnboundedSender<Incoming>,
    ) -> anyhow::Result<Self> {
        let Spawned { child, stdin, stdout } = super::process::spawn(launch, &rings)?;
        let transport = ByteStreams::new(
            TapWrite { inner: stdin, rings: Arc::clone(&rings), line: Vec::new() }.compat_write(),
            TapRead { inner: stdout, rings: Arc::clone(&rings), line: Vec::new() }.compat(),
        );
        let (cx_tx, cx_rx) = oneshot::channel::<ConnectionTo<Agent>>();
        let (close_tx, close_rx) = oneshot::channel::<()>();
        let notify_inbox = inbox.clone();
        let request_inbox = inbox.clone();
        let builder = Client
            .builder()
            .name("cctui-acp")
            .on_receive_notification(
                async move |n: UntypedMessage, _cx: ConnectionTo<Agent>| {
                    let _ = notify_inbox
                        .send(Incoming::Notification { method: n.method, params: n.params });
                    Ok(())
                },
                agent_client_protocol::on_receive_notification!(),
            )
            .on_receive_request(
                async move |req: UntypedMessage,
                            responder: Responder<Value>,
                            cx: ConnectionTo<Agent>| {
                    if req.method != "session/request_permission" {
                        return responder.respond_with_error(
                            agent_client_protocol::Error::method_not_found().data(req.method),
                        );
                    }
                    let (reply, answer) = oneshot::channel::<Value>();
                    if request_inbox
                        .send(Incoming::Permission { params: req.params, reply })
                        .is_err()
                    {
                        return responder.respond(cancelled());
                    }
                    cx.spawn(async move {
                        responder.respond(answer.await.unwrap_or_else(|_| cancelled()))
                    })
                },
                agent_client_protocol::on_receive_request!(),
            );
        let closed_inbox = inbox;
        let closed_rings = Arc::clone(&rings);
        tokio::spawn(async move {
            let run = builder.connect_with(transport, async move |cx: ConnectionTo<Agent>| {
                let _ = cx_tx.send(cx.clone());
                tokio::select! {
                    () = cx.incoming_closed() => Err(agent_client_protocol::Error::internal_error()
                        .data(serde_json::json!({ "reason": "agent closed its protocol stream" }))),
                    _ = close_rx => Ok(()),
                }
            });
            let detail = match run.await {
                Ok(()) => None,
                Err(err) => {
                    let text = rpc_error_text(&err);
                    closed_rings.note_protocol_error(&text);
                    Some(text)
                }
            };
            let _ = closed_inbox.send(Incoming::Closed { detail });
        });
        let cx = cx_rx.await.map_err(|_| {
            anyhow::anyhow!("the agent's protocol stream closed before the handshake")
        })?;
        Ok(Self { requester: Requester { cx }, close: Some(close_tx), child })
    }

    #[must_use]
    pub fn requester(&self) -> Requester {
        self.requester.clone()
    }

    pub async fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> anyhow::Result<Value> {
        self.requester.request(method, params, timeout).await
    }

    pub fn notify(&self, method: &str, params: Value) -> anyhow::Result<()> {
        self.requester.notify(method, params)
    }

    /// Stop the protocol task; the process is the caller's to terminate.
    pub fn close(&mut self) {
        if let Some(close) = self.close.take() {
            let _ = close.send(());
        }
    }
}

fn cancelled() -> Value {
    serde_json::json!({ "outcome": { "outcome": "cancelled" } })
}

fn rpc_error_text(err: &agent_client_protocol::Error) -> String {
    match &err.data {
        Some(data) if !data.is_null() => format!("{} {data}", err.message),
        _ => err.message.clone(),
    }
}

/// Feed complete lines into the rpc ring, labelled by their method or id.
fn note_lines(line: &mut Vec<u8>, bytes: &[u8], rings: &TrafficRings, direction: &str) {
    for &b in bytes {
        if b == b'\n' {
            let text = String::from_utf8_lossy(line);
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                match serde_json::from_str::<Value>(trimmed) {
                    Ok(value) => rings.note_rpc(direction, &value),
                    Err(err) => {
                        rings
                            .note_protocol_error(&format!("{direction}: unparseable frame: {err}"));
                    }
                }
            }
            line.clear();
        } else if line.len() < TAP_LINE_MAX {
            line.push(b);
        }
    }
}

struct TapRead<R> {
    inner: R,
    rings: Arc<TrafficRings>,
    line: Vec<u8>,
}

impl<R: AsyncRead + Unpin> AsyncRead for TapRead<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let before = buf.filled().len();
        let polled = Pin::new(&mut this.inner).poll_read(cx, buf);
        if matches!(&polled, Poll::Ready(Ok(()))) {
            let fresh = &buf.filled()[before..];
            note_lines(&mut this.line, fresh, &this.rings, "in");
        }
        polled
    }
}

struct TapWrite<W> {
    inner: W,
    rings: Arc<TrafficRings>,
    line: Vec<u8>,
}

impl<W: AsyncWrite + Unpin> AsyncWrite for TapWrite<W> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        let polled = Pin::new(&mut this.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(n)) = &polled {
            note_lines(&mut this.line, &buf[..*n], &this.rings, "out");
        }
        polled
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tap_labels_frames_by_method_and_notes_garbage() {
        let rings = TrafficRings::default();
        let mut line = Vec::new();
        note_lines(
            &mut line,
            b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}\nnot json\n{\"id\":1,",
            &rings,
            "out",
        );
        let rpc = rings.rpc_tail();
        assert_eq!(rpc.len(), 1);
        assert_eq!(rpc[0].label, "initialize");
        assert_eq!(rpc[0].direction, "out");
        assert_eq!(rings.protocol_errors().len(), 1);
        assert_eq!(line, b"{\"id\":1,");
        note_lines(&mut line, b"\"result\":{}}\n", &rings, "in");
        assert_eq!(rings.rpc_tail()[1].label, "1");
        assert!(line.is_empty());
    }

    #[test]
    fn an_endless_line_is_bounded() {
        let rings = TrafficRings::default();
        let mut line = Vec::new();
        note_lines(&mut line, &vec![b'x'; TAP_LINE_MAX + 10], &rings, "in");
        assert_eq!(line.len(), TAP_LINE_MAX);
    }
}
