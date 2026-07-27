//! Read-only live PTY relay.
//!
//! When a browser opens a session's terminal view, the server sends
//! `AdapterCommand::WatchPty { watch: true }`. Rather than tapping the held
//! keep-alive attach (`attach.rs`, which is mid-stream), we open a *fresh*
//! attach dedicated to that viewer: a fresh attach makes the worker repaint the
//! full current screen, so a mid-session viewer gets the current frame for free
//! with no server-side VT state or replay buffer. Post-ack raw PTY bytes are
//! coalesced, base64-encoded, and emitted as `AdapterEvent::PtyChunk` — the same
//! event pump the rest of the adapter uses. We NEVER write to the socket after
//! the request (any bytes written would be injected as keystrokes); the viewer
//! is strictly read-only. Closing the view (`watch: false`) cancels the task,
//! dropping the extra attacher so it can't block the worker's idle-retire.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use cctui_proto::adapter::AdapterEvent;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::attach::attach_request;
use super::discovery::Discovery;

/// Bytes buffered before a coalesced frame is flushed regardless of the timer.
/// Bounds per-frame size so one repaint burst can't produce a huge base64
/// payload; a normal TUI repaint is a few KB.
const MAX_FRAME_BYTES: usize = 16 * 1024;

/// Coalescing window: rapid small PTY writes accumulate for this long before one
/// frame is emitted, so a spinner ticking at ~KB/s doesn't produce hundreds of
/// tiny WS frames.
const FLUSH_INTERVAL: Duration = Duration::from_millis(40);

/// Backoff before re-dialing the viewer attach after a clean detach while still
/// watched (worker settled/respawned under the same short).
const RECONNECT_BACKOFF: Duration = Duration::from_millis(500);

const READ_BUF: usize = 8192;

/// Size-bounded byte accumulator that coalesces PTY reads into frames. `push`
/// splits off full `max`-sized frames immediately; the sub-`max` remainder is
/// drained by `take` on the flush tick.
pub(super) struct Coalescer {
    buf: Vec<u8>,
    max: usize,
}

impl Coalescer {
    pub(super) const fn new(max: usize) -> Self {
        Self { buf: Vec::new(), max }
    }

    /// Append `data`, returning every full `max`-sized frame it completes.
    pub(super) fn push(&mut self, data: &[u8]) -> Vec<Vec<u8>> {
        self.buf.extend_from_slice(data);
        let mut frames = Vec::new();
        while self.buf.len() >= self.max {
            let rest = self.buf.split_off(self.max);
            frames.push(std::mem::replace(&mut self.buf, rest));
        }
        frames
    }

    /// Take the buffered remainder as a frame, or `None` when empty.
    pub(super) fn take(&mut self) -> Option<Vec<u8>> {
        if self.buf.is_empty() { None } else { Some(std::mem::take(&mut self.buf)) }
    }
}

/// Owns one viewer-attach task per watched `short`, started/stopped by the
/// `WatchPty` command. Cloneable + interior-mutable so the `&self`
/// `handle_command` path can drive it.
#[derive(Clone)]
pub(super) struct PtyViewManager {
    events: mpsc::Sender<AdapterEvent>,
    discovery: Discovery,
    shutdown: CancellationToken,
    tasks: Arc<Mutex<HashMap<String, CancellationToken>>>,
}

impl PtyViewManager {
    pub(super) fn new(
        events: mpsc::Sender<AdapterEvent>,
        discovery: Discovery,
        shutdown: CancellationToken,
    ) -> Self {
        Self { events, discovery, shutdown, tasks: Arc::new(Mutex::new(HashMap::new())) }
    }

    /// Begin forwarding `short`'s PTY as `PtyChunk` events tagged `local_id`.
    /// Idempotent — a watch for a short already streaming is a no-op.
    pub(super) fn watch(&self, local_id: String, short: String) {
        let Ok(mut tasks) = self.tasks.lock() else { return };
        if tasks.contains_key(&short) {
            return;
        }
        let cancel = self.shutdown.child_token();
        let task = PtyViewTask {
            events: self.events.clone(),
            discovery: self.discovery.clone(),
            short: short.clone(),
            local_id,
            cancel: cancel.clone(),
        };
        tokio::spawn(task.run());
        tasks.insert(short, cancel);
        tracing::debug!(watching = tasks.len(), "pty view started");
    }

    /// Stop forwarding `short` and drop the viewer attach.
    pub(super) fn unwatch(&self, short: &str) {
        if let Ok(mut tasks) = self.tasks.lock()
            && let Some(cancel) = tasks.remove(short)
        {
            cancel.cancel();
        }
    }
}

struct PtyViewTask {
    events: mpsc::Sender<AdapterEvent>,
    discovery: Discovery,
    short: String,
    local_id: String,
    cancel: CancellationToken,
}

impl PtyViewTask {
    async fn run(self) {
        while !self.cancel.is_cancelled() {
            if let Err(err) = self.stream_once().await {
                tracing::debug!(short = %self.short, %err, "pty view attach cycle ended");
            }
            // Clean detach or a transient failure: re-dial while still watched so
            // a respawn under the same short keeps the viewer live.
            tokio::select! {
                () = self.cancel.cancelled() => return,
                () = tokio::time::sleep(RECONNECT_BACKOFF) => {}
            }
        }
    }

    /// One dial → attach → forward-until-EOF cycle.
    async fn stream_once(&self) -> anyhow::Result<()> {
        let Some(sock) = self.discovery.locate_live().await else {
            anyhow::bail!("no live claude daemon socket");
        };
        let stream = UnixStream::connect(&sock).await?;
        let (read_half, mut write_half) = stream.into_split();

        let attach_id = uuid::Uuid::new_v4().simple().to_string();
        let req = attach_request(&self.short, &attach_id);
        let mut line = serde_json::to_string(&req)?;
        line.push('\n');
        write_half.write_all(line.as_bytes()).await?;
        write_half.flush().await?;

        let mut reader = BufReader::new(read_half);
        let mut ack = String::new();
        if reader.read_line(&mut ack).await? == 0 {
            anyhow::bail!("eof before attach ack");
        }
        let ack: Value = serde_json::from_str(ack.trim())?;
        if ack.get("ok") == Some(&Value::Bool(false)) {
            let code = ack.get("code").and_then(Value::as_str).unwrap_or("?");
            anyhow::bail!("viewer attach rejected: {code}");
        }

        // Post-ack: raw PTY bytes. From here we only ever read.
        let mut coalescer = Coalescer::new(MAX_FRAME_BYTES);
        let mut buf = [0_u8; READ_BUF];
        let mut flush = tokio::time::interval(FLUSH_INTERVAL);
        flush.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        flush.tick().await;
        loop {
            tokio::select! {
                () = self.cancel.cancelled() => return Ok(()),
                _ = flush.tick() => {
                    if let Some(frame) = coalescer.take()
                        && !self.emit(&frame) {
                        return Ok(());
                    }
                }
                read = reader.read(&mut buf) => {
                    match read? {
                        0 => return Ok(()), // server FIN — detached / settled
                        len => {
                            for frame in coalescer.push(&buf[..len]) {
                                if !self.emit(&frame) {
                                    return Ok(());
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// Base64-encode + emit one coalesced frame. Uses `try_send` so a slow
    /// browser can't stall the shared event channel (backpressure → drop, never
    /// queue). Returns `false` only when the channel is permanently
    /// closed (adapter shutting down) so the caller stops.
    fn emit(&self, frame: &[u8]) -> bool {
        let data = base64::engine::general_purpose::STANDARD.encode(frame);
        match self.events.try_send(AdapterEvent::PtyChunk { local_id: self.local_id.clone(), data })
        {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                tracing::debug!(short = %self.short, "pty chunk dropped (event channel full)");
                true
            }
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A byte run larger than `max` splits into full frames on `push`, and the
    /// sub-`max` remainder is only surfaced by `take` (the flush-tick path).
    #[test]
    fn coalescer_splits_full_frames_and_flushes_remainder() {
        let mut c = Coalescer::new(4);
        // 10 bytes @ max 4 → two full [4] frames on push, 2 left buffered.
        let frames = c.push(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        assert_eq!(frames, vec![vec![1, 2, 3, 4], vec![5, 6, 7, 8]]);
        assert_eq!(c.take(), Some(vec![9, 10]));
        assert_eq!(c.take(), None, "buffer drained");
    }

    /// Small writes accumulate without emitting until the flush tick drains
    /// them — the coalescing that keeps a spinner from flooding the WS.
    #[test]
    fn coalescer_accumulates_small_writes() {
        let mut c = Coalescer::new(16);
        assert!(c.push(b"ab").is_empty());
        assert!(c.push(b"cd").is_empty());
        assert_eq!(c.take(), Some(b"abcd".to_vec()));
    }

    /// An exact multiple of `max` emits whole frames with nothing left over.
    #[test]
    fn coalescer_exact_multiple_leaves_no_remainder() {
        let mut c = Coalescer::new(3);
        let frames = c.push(&[0, 1, 2, 3, 4, 5]);
        assert_eq!(frames, vec![vec![0, 1, 2], vec![3, 4, 5]]);
        assert_eq!(c.take(), None);
    }

    /// `watch` is idempotent per short (a re-sent `watch: true` doesn't stack a
    /// second viewer attach), and `unwatch` cancels + removes the task.
    #[tokio::test]
    async fn watch_is_idempotent_and_unwatch_cancels() {
        let dir = tempfile::tempdir().unwrap();
        let (events, _rx) = mpsc::channel(8);
        let mgr = PtyViewManager::new(
            events,
            Discovery::with_base(dir.path().to_path_buf()),
            CancellationToken::new(),
        );

        mgr.watch("sess-1".to_owned(), "aaaaaaaa".to_owned());
        mgr.watch("sess-1".to_owned(), "aaaaaaaa".to_owned());
        assert_eq!(mgr.tasks.lock().unwrap().len(), 1, "same short must not stack tasks");

        let token = mgr.tasks.lock().unwrap()["aaaaaaaa"].clone();
        mgr.unwatch("aaaaaaaa");
        assert!(token.is_cancelled(), "unwatch must cancel the task");
        assert!(mgr.tasks.lock().unwrap().is_empty());
    }
}
