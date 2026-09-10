//! Read-only live view of a codex session's protocol traffic.
//!
//! Codex is headless, so there is no PTY to attach to: the watch streams the
//! diagnose rings as text lines through `AdapterEvent::PtyChunk` instead, which
//! is what lets `TerminalPane` and the ws relay work unchanged.
//!
//! The rings are reachable only through `SessionCommand::Diagnose`, so the
//! watcher polls it and emits the suffix that is new. Polling keeps this
//! entirely off the RPC path — a viewer cannot slow a turn down.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use cctui_proto::adapter::AdapterEvent;
use cctui_proto::diagnose::{CodexProtocolError, CodexRpcFrame, CodexStderrLine};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::app_server::{CodexLiveSnapshot, LiveSessionRegistry, SessionCommand};

const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// One viewer task per watched `local_id`, started and stopped by `WatchPty`.
#[derive(Clone, Default)]
pub(super) struct RingViewManager {
    tasks: Arc<Mutex<HashMap<String, CancellationToken>>>,
}

impl RingViewManager {
    /// Idempotent: a second watch for an already-streaming session is a no-op.
    pub(super) fn watch(
        &self,
        local_id: String,
        live: LiveSessionRegistry,
        events: mpsc::Sender<AdapterEvent>,
        shutdown: &CancellationToken,
    ) {
        let Ok(mut tasks) = self.tasks.lock() else { return };
        if tasks.contains_key(&local_id) {
            return;
        }
        let cancel = shutdown.child_token();
        tasks.insert(local_id.clone(), cancel.clone());
        tokio::spawn(stream(local_id, live, events, cancel));
    }

    pub(super) fn unwatch(&self, local_id: &str) {
        if let Ok(mut tasks) = self.tasks.lock()
            && let Some(cancel) = tasks.remove(local_id)
        {
            cancel.cancel();
        }
    }
}

/// Cursors into the three rings: the last entry already emitted, so a poll
/// emits the suffix after it. A cursor that no longer appears (the ring rolled
/// past it between polls) replays the whole tail rather than losing frames.
#[derive(Default)]
struct Cursor {
    rpc: Option<CodexRpcFrame>,
    stderr: Option<CodexStderrLine>,
    errors: Option<CodexProtocolError>,
}

fn since<T: PartialEq + Clone>(tail: &[T], cursor: &mut Option<T>) -> Vec<T> {
    let start = cursor.as_ref().and_then(|c| tail.iter().position(|e| e == c)).map_or(0, |i| i + 1);
    let fresh = tail[start.min(tail.len())..].to_vec();
    if let Some(last) = fresh.last() {
        *cursor = Some(last.clone());
    }
    fresh
}

fn hhmmss(ts_ms: i64) -> String {
    let secs = ts_ms.div_euclid(1000);
    let (h, m, s) = (secs.div_euclid(3600) % 24, secs.div_euclid(60) % 60, secs.rem_euclid(60));
    format!("{h:02}:{m:02}:{s:02}.{:03}", ts_ms.rem_euclid(1000))
}

/// Terminal lines, CRLF-terminated because the browser pane is a raw VT.
fn render(snapshot: &CodexLiveSnapshot, cursor: &mut Cursor) -> String {
    let mut out = String::new();
    for line in since(&snapshot.stderr_tail, &mut cursor.stderr) {
        let _ = writeln!(out, "{}  stderr  {}\r", hhmmss(line.ts_ms), line.line);
    }
    for err in since(&snapshot.protocol_errors, &mut cursor.errors) {
        let _ = writeln!(out, "{}  [{}] !! {}\r", hhmmss(err.ts_ms), err.transport, err.message);
    }
    for frame in since(&snapshot.rpc_tail, &mut cursor.rpc) {
        let arrow = if frame.direction == "out" { "->" } else { "<-" };
        let _ = writeln!(
            out,
            "{}  [{}] {arrow} {}  {}\r",
            hhmmss(frame.ts_ms),
            frame.transport,
            frame.label,
            frame.json
        );
    }
    out
}

async fn snapshot(live: &LiveSessionRegistry, local_id: &str) -> Option<CodexLiveSnapshot> {
    let tx = live.lock().await.get(local_id).cloned()?;
    let (reply, mut rx) = mpsc::channel(1);
    tx.send(SessionCommand::Diagnose { reply }).await.ok()?;
    tokio::time::timeout(POLL_INTERVAL, rx.recv()).await.ok().flatten()
}

async fn stream(
    local_id: String,
    live: LiveSessionRegistry,
    events: mpsc::Sender<AdapterEvent>,
    cancel: CancellationToken,
) {
    let mut cursor = Cursor::default();
    loop {
        tokio::select! {
            () = cancel.cancelled() => return,
            () = tokio::time::sleep(POLL_INTERVAL) => {}
        }
        let Some(snap) = snapshot(&live, &local_id).await else { continue };
        let text = render(&snap, &mut cursor);
        if text.is_empty() {
            continue;
        }
        let data = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
        if events.send(AdapterEvent::PtyChunk { local_id: local_id.clone(), data }).await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(ts_ms: i64, transport: &str) -> CodexRpcFrame {
        CodexRpcFrame {
            ts_ms,
            direction: "out".to_owned(),
            label: "thread/list".to_owned(),
            json: format!("{{\"id\":{ts_ms}}}"),
            transport: transport.to_owned(),
        }
    }

    #[test]
    fn a_poll_emits_only_what_is_new() {
        let mut cursor = Cursor::default();
        let mut snap = CodexLiveSnapshot {
            rpc_tail: vec![frame(1000, "stdio"), frame(2000, "shared")],
            ..CodexLiveSnapshot::default()
        };
        let first = render(&snap, &mut cursor);
        assert!(first.contains("[stdio] -> thread/list"), "{first}");
        assert!(first.contains("[shared] -> thread/list"), "{first}");
        assert_eq!(render(&snap, &mut cursor), "");

        snap.rpc_tail.push(frame(3000, "shared"));
        let next = render(&snap, &mut cursor);
        assert_eq!(next.lines().count(), 1, "{next}");
        assert!(next.contains("{\"id\":3000}"), "{next}");
    }

    #[test]
    fn a_rolled_past_cursor_replays_the_tail_rather_than_losing_it() {
        let mut cursor = Cursor { rpc: Some(frame(1, "stdio")), ..Cursor::default() };
        let snap = CodexLiveSnapshot {
            rpc_tail: vec![frame(9000, "shared")],
            ..CodexLiveSnapshot::default()
        };
        assert!(render(&snap, &mut cursor).contains("9000"));
    }

    #[test]
    fn protocol_errors_and_stderr_are_streamed_with_their_transport() {
        let mut cursor = Cursor::default();
        let snap = CodexLiveSnapshot {
            stderr_tail: vec![CodexStderrLine { ts_ms: 1000, line: "boom".to_owned() }],
            protocol_errors: vec![CodexProtocolError {
                ts_ms: 1000,
                message: "connection dropped before request 7 was answered".to_owned(),
                transport: "shared".to_owned(),
            }],
            ..CodexLiveSnapshot::default()
        };
        let text = render(&snap, &mut cursor);
        assert!(text.contains("stderr  boom"), "{text}");
        assert!(text.contains("[shared] !! connection dropped"), "{text}");
    }
}
