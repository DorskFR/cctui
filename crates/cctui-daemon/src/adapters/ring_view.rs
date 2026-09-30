//! Read-only live view of a headless harness's protocol traffic.
//!
//! Codex and opencode have no PTY to attach to, so a watch streams their
//! [`crate::adapters::traffic_rings`] entries as text lines through
//! `AdapterEvent::PtyChunk` instead, which is what lets `TerminalPane` and the
//! ws relay work unchanged.
//!
//! The rings are reachable only through each driver's own diagnose command, so
//! the watcher polls it and emits the suffix that is new. Polling keeps this
//! entirely off the protocol path — a viewer cannot slow a turn down.

use std::fmt::Write as _;
use std::future::Future;
use std::time::Duration;

use base64::Engine as _;
use cctui_proto::adapter::AdapterEvent;
use cctui_proto::diagnose::{TrafficError, TrafficFrame, TrafficStderrLine};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::adapters::pty_watch::PtyWatchSet;

pub const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// What a driver hands the viewer each poll: the three ring tails, whatever
/// their transports.
#[derive(Debug, Clone, Default)]
pub struct TrafficSnapshot {
    pub protocol_errors: Vec<TrafficError>,
    pub stderr_tail: Vec<TrafficStderrLine>,
    pub rpc_tail: Vec<TrafficFrame>,
}

/// Cursors into the three rings: the last entry already emitted.
///
/// A poll emits the suffix after it. A cursor that no longer appears (the ring
/// rolled past it between polls) replays the whole tail rather than losing
/// frames.
#[derive(Default)]
pub struct Cursor {
    rpc: Option<TrafficFrame>,
    stderr: Option<TrafficStderrLine>,
    errors: Option<TrafficError>,
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

/// The raw JSON of a frame is one long line; a pane full of them is unreadable.
/// Keep a bounded prefix and collapse the whitespace a pretty-printed body
/// carries, so one frame stays one scannable line.
const BODY_MAX: usize = 400;

fn one_line(json: &str) -> String {
    let mut out = String::with_capacity(json.len().min(BODY_MAX));
    let mut space = false;
    for ch in json.chars() {
        if ch.is_whitespace() {
            space = true;
            continue;
        }
        if space && !out.is_empty() {
            out.push(' ');
        }
        space = false;
        out.push(ch);
        if out.chars().count() >= BODY_MAX {
            out.push('…');
            break;
        }
    }
    out
}

/// Terminal lines, CRLF-terminated because the browser pane is a raw VT.
pub fn render(snapshot: &TrafficSnapshot, cursor: &mut Cursor) -> String {
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
            one_line(&frame.json)
        );
    }
    out
}

/// One viewer task per watched `local_id`, started and stopped by `WatchPty`.
///
/// `poll` returns the driver's current snapshot, or `None` while no live
/// session owns that id — a watch may legitimately arrive before its session.
#[derive(Clone, Default)]
pub struct RingViewManager {
    watches: PtyWatchSet,
}

impl RingViewManager {
    /// Idempotent: a second watch for an already-streaming session is a no-op.
    pub fn watch<P, Fut>(
        &self,
        local_id: String,
        events: mpsc::Sender<AdapterEvent>,
        shutdown: &CancellationToken,
        poll: P,
    ) where
        P: Fn(String) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Option<TrafficSnapshot>> + Send + 'static,
    {
        let key = local_id.clone();
        self.watches.watch(key, shutdown, move |cancel| stream(local_id, events, cancel, poll));
    }

    pub fn unwatch(&self, local_id: &str) {
        self.watches.unwatch(local_id);
    }
}

async fn stream<P, Fut>(
    local_id: String,
    events: mpsc::Sender<AdapterEvent>,
    cancel: CancellationToken,
    poll: P,
) where
    P: Fn(String) -> Fut + Send + Sync,
    Fut: Future<Output = Option<TrafficSnapshot>> + Send + 'static,
{
    let mut cursor = Cursor::default();
    loop {
        tokio::select! {
            () = cancel.cancelled() => return,
            () = tokio::time::sleep(POLL_INTERVAL) => {}
        }
        let Some(snap) = poll(local_id.clone()).await else { continue };
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

    fn frame(ts_ms: i64, transport: &str) -> TrafficFrame {
        TrafficFrame {
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
        let mut snap = TrafficSnapshot {
            rpc_tail: vec![frame(1000, "stdio"), frame(2000, "shared")],
            ..TrafficSnapshot::default()
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
        let snap =
            TrafficSnapshot { rpc_tail: vec![frame(9000, "shared")], ..TrafficSnapshot::default() };
        assert!(render(&snap, &mut cursor).contains("9000"));
    }

    #[test]
    fn protocol_errors_and_stderr_are_streamed_with_their_transport() {
        let mut cursor = Cursor::default();
        let snap = TrafficSnapshot {
            stderr_tail: vec![TrafficStderrLine { ts_ms: 1000, line: "boom".to_owned() }],
            protocol_errors: vec![TrafficError {
                ts_ms: 1000,
                message: "connection dropped before request 7 was answered".to_owned(),
                transport: "shared".to_owned(),
            }],
            ..TrafficSnapshot::default()
        };
        let text = render(&snap, &mut cursor);
        assert!(text.contains("stderr  boom"), "{text}");
        assert!(text.contains("[shared] !! connection dropped"), "{text}");
    }

    /// A pretty-printed SSE payload would otherwise wrap over dozens of pane
    /// rows and bury every neighbouring frame.
    #[test]
    fn a_frame_body_stays_one_bounded_line() {
        let mut cursor = Cursor::default();
        let mut big = frame(1000, "sse");
        big.json = format!("{{\n  \"blob\": \"{}\"\n}}", "x".repeat(2000));
        let snap = TrafficSnapshot { rpc_tail: vec![big], ..TrafficSnapshot::default() };

        let text = render(&snap, &mut cursor);
        assert_eq!(text.lines().count(), 1, "{text}");
        assert!(text.contains('…'), "{text}");
        assert!(text.chars().count() < BODY_MAX + 80, "{}", text.chars().count());
        assert!(!text.contains("\n  "), "{text}");
    }
}
