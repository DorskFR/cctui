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

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use base64::Engine as _;
use cctui_proto::adapter::AdapterEvent;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::attach::attach_request;
use super::discovery::Discovery;
use super::roster::SessionRoster;
use crate::adapter_runtime::PtyWatch;
use crate::adapters::pty_watch::PtyWatchSet;

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

/// The repaint that follows a fresh attach is the ONLY way a viewer learns the
/// current screen, so it is worth waiting on a congested event channel for —
/// dropping it leaves the pane blank until the TUI redraws on its own, which on
/// an idle session can take tens of seconds. Steady-state frames keep dropping.
const FIRST_FRAME_SEND_TIMEOUT: Duration = Duration::from_secs(2);

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

/// One viewer-attach task per watched `short`, started/stopped by the
/// `WatchPty` command.
#[derive(Clone)]
pub(super) struct PtyViewManager {
    events: mpsc::Sender<AdapterEvent>,
    discovery: Discovery,
    shutdown: CancellationToken,
    watches: PtyWatchSet,
}

impl PtyViewManager {
    pub(super) fn new(
        events: mpsc::Sender<AdapterEvent>,
        discovery: Discovery,
        shutdown: CancellationToken,
    ) -> Self {
        Self { events, discovery, shutdown, watches: PtyWatchSet::default() }
    }

    /// Begin forwarding `short`'s PTY as `PtyChunk` events tagged `local_id`.
    /// Idempotent — a watch for a short already streaming is a no-op.
    /// `requested_at` is when the `WatchPty` arrived, so the paint latency the
    /// user actually sees is what gets logged.
    pub(super) fn watch(&self, local_id: String, short: String, requested_at: Instant) {
        let (events, discovery) = (self.events.clone(), self.discovery.clone());
        let task_short = short.clone();
        let started = self.watches.watch(short.clone(), &self.shutdown, move |cancel| {
            PtyViewTask { events, discovery, short: task_short, local_id, cancel, requested_at }
                .run()
        });
        if started {
            tracing::info!(
                %short,
                watching = self.watches.watching(),
                queued_ms = requested_at.elapsed().as_millis(),
                "pty view started",
            );
        }
    }

    /// Stop forwarding `short` and drop the viewer attach.
    pub(super) fn unwatch(&self, short: &str) {
        self.watches.unwatch(short);
    }
}

/// Drains the adapter's out-of-band `WatchPty` channel. It shares nothing with
/// the serial command loop, so a watch is served while a `reply` is still in its
/// 30s settle/confirm loop.
pub(super) struct PtyWatchPump {
    views: PtyViewManager,
    roster: SessionRoster,
    shutdown: CancellationToken,
}

impl PtyWatchPump {
    pub(super) const fn new(
        views: PtyViewManager,
        roster: SessionRoster,
        shutdown: CancellationToken,
    ) -> Self {
        Self { views, roster, shutdown }
    }

    pub(super) async fn run(self, mut watches: mpsc::Receiver<PtyWatch>) {
        // A watch for a session the roster doesn't list yet (just spawned, or
        // before the first `list` after a daemon restart) parks here instead of
        // being dropped: the browser never re-sends until a WS reconnect.
        let mut pending: HashSet<String> = HashSet::new();
        // The short each started watch is keyed under. Remembered rather than
        // re-resolved, so a `watch: false` still stops the viewer after the
        // session has already dropped off the roster.
        let mut started: HashMap<String, String> = HashMap::new();
        loop {
            tokio::select! {
                () = self.shutdown.cancelled() => return,
                watch = watches.recv() => {
                    let Some((local_id, watch)) = watch else { return };
                    self.apply(local_id, watch, &mut pending, &mut started);
                }
                () = self.roster.changed(), if !pending.is_empty() => {
                    pending.retain(|local_id| {
                        !self.start(local_id.clone(), Instant::now(), &mut started)
                    });
                }
            }
        }
    }

    fn apply(
        &self,
        local_id: String,
        watch: bool,
        pending: &mut HashSet<String>,
        started: &mut HashMap<String, String>,
    ) {
        if !watch {
            pending.remove(&local_id);
            if let Some(short) = started.remove(&local_id) {
                self.views.unwatch(&short);
            }
            return;
        }
        if !self.start(local_id.clone(), Instant::now(), started) {
            tracing::info!(%local_id, "pty watch pending; session not yet rostered");
            pending.insert(local_id);
        }
    }

    /// Start a viewer if `local_id` resolves. Returns whether it did.
    fn start(
        &self,
        local_id: String,
        requested_at: Instant,
        started: &mut HashMap<String, String>,
    ) -> bool {
        let Some(short) = self.roster.get(&local_id) else { return false };
        started.insert(local_id.clone(), short.clone());
        self.views.watch(local_id, short, requested_at);
        true
    }
}

struct PtyViewTask {
    events: mpsc::Sender<AdapterEvent>,
    discovery: Discovery,
    short: String,
    local_id: String,
    cancel: CancellationToken,
    requested_at: Instant,
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
        tracing::info!(
            short = %self.short,
            attach_ms = self.requested_at.elapsed().as_millis(),
            "pty view attach acked",
        );

        // Post-ack: raw PTY bytes. From here we only ever read.
        let mut first = true;
        let mut coalescer = Coalescer::new(MAX_FRAME_BYTES);
        let mut buf = [0_u8; READ_BUF];
        let mut flush = tokio::time::interval(FLUSH_INTERVAL);
        flush.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        flush.tick().await;
        loop {
            tokio::select! {
                () = self.cancel.cancelled() => return Ok(()),
                _ = flush.tick() => {
                    if let Some(frame) = coalescer.take() {
                        if !self.emit(&frame, first).await {
                            return Ok(());
                        }
                        first = false;
                    }
                }
                read = reader.read(&mut buf) => {
                    match read? {
                        0 => return Ok(()), // server FIN — detached / settled
                        len => {
                            for frame in coalescer.push(&buf[..len]) {
                                if !self.emit(&frame, first).await {
                                    return Ok(());
                                }
                                first = false;
                            }
                        }
                    }
                }
            }
        }
    }

    /// Base64-encode + emit one coalesced frame. A steady-state frame uses
    /// `try_send` so a slow browser can't stall the shared event channel
    /// (backpressure → drop, never queue); the post-attach repaint (`first`)
    /// gets a bounded `send` instead, because losing it blanks the pane.
    /// Returns `false` only when the channel is permanently closed (adapter
    /// shutting down) so the caller stops.
    async fn emit(&self, frame: &[u8], first: bool) -> bool {
        let data = base64::engine::general_purpose::STANDARD.encode(frame);
        let event = AdapterEvent::PtyChunk { local_id: self.local_id.clone(), data };
        if first {
            return match tokio::time::timeout(FIRST_FRAME_SEND_TIMEOUT, self.events.send(event))
                .await
            {
                Ok(Ok(())) => {
                    tracing::info!(
                        short = %self.short,
                        first_frame_ms = self.requested_at.elapsed().as_millis(),
                        "pty view first frame emitted",
                    );
                    true
                }
                Ok(Err(_)) => false,
                Err(_) => {
                    tracing::warn!(
                        short = %self.short,
                        "pty view first frame dropped (event channel full for {FIRST_FRAME_SEND_TIMEOUT:?})",
                    );
                    true
                }
            };
        }
        match self.events.try_send(event) {
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
    use base64::Engine as _;

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

        mgr.watch("sess-1".to_owned(), "aaaaaaaa".to_owned(), Instant::now());
        mgr.watch("sess-1".to_owned(), "aaaaaaaa".to_owned(), Instant::now());
        assert_eq!(mgr.watches.watching(), 1, "same short must not stack tasks");

        let token = mgr.watches.token("aaaaaaaa").unwrap();
        mgr.unwatch("aaaaaaaa");
        assert!(token.is_cancelled(), "unwatch must cancel the task");
        assert_eq!(mgr.watches.watching(), 0);
    }

    struct PumpParts {
        views: PtyViewManager,
        roster: SessionRoster,
        shutdown: CancellationToken,
        _events_rx: mpsc::Receiver<AdapterEvent>,
        _dir: tempfile::TempDir,
    }

    fn pump_parts() -> PumpParts {
        let dir = tempfile::tempdir().unwrap();
        let (events, events_rx) = mpsc::channel(8);
        let shutdown = CancellationToken::new();
        let views = PtyViewManager::new(
            events,
            Discovery::with_base(dir.path().to_path_buf()),
            shutdown.clone(),
        );
        PumpParts {
            views,
            roster: SessionRoster::default(),
            shutdown,
            _events_rx: events_rx,
            _dir: dir,
        }
    }

    /// The watch set only ever grows to 1 within a poll window; spin rather
    /// than sleep a fixed time so the test is not timing-fragile.
    async fn wait_for_watching(views: &PtyViewManager, want: usize) {
        for _ in 0..200 {
            if views.watches.watching() == want {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(views.watches.watching(), want, "watch count never reached {want}");
    }

    /// A watch that arrives before the session is rostered parks, and starts
    /// streaming as soon as a `list` snapshot inserts its short.
    #[tokio::test]
    async fn a_watch_for_an_unrostered_session_starts_once_the_roster_lists_it() {
        let parts = pump_parts();
        let (tx, rx) = mpsc::channel(4);
        let pump =
            PtyWatchPump::new(parts.views.clone(), parts.roster.clone(), parts.shutdown.clone());
        let handle = tokio::spawn(pump.run(rx));

        tx.send(("sess-1".to_owned(), true)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            parts.views.watches.watching(),
            0,
            "an unrostered watch must not start a viewer"
        );

        parts.roster.insert("sess-1".to_owned(), "aaaaaaaa".to_owned());
        wait_for_watching(&parts.views, 1).await;

        parts.shutdown.cancel();
        let _ = handle.await;
    }

    /// `watch: false` while still pending cancels it, so a later roster insert
    /// does not resurrect a view the browser already closed.
    #[tokio::test]
    async fn unwatching_a_pending_watch_cancels_it() {
        let parts = pump_parts();
        let (tx, rx) = mpsc::channel(4);
        let pump =
            PtyWatchPump::new(parts.views.clone(), parts.roster.clone(), parts.shutdown.clone());
        let handle = tokio::spawn(pump.run(rx));

        tx.send(("sess-1".to_owned(), true)).await.unwrap();
        tx.send(("sess-1".to_owned(), false)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        parts.roster.insert("sess-1".to_owned(), "aaaaaaaa".to_owned());
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(parts.views.watches.watching(), 0, "a cancelled pending watch must not start");

        parts.shutdown.cancel();
        let _ = handle.await;
    }

    /// The pump stops a viewer whose session has already left the roster — the
    /// short it started under is remembered, not re-resolved.
    #[tokio::test]
    async fn unwatch_stops_a_viewer_after_the_session_left_the_roster() {
        let parts = pump_parts();
        let (tx, rx) = mpsc::channel(4);
        parts.roster.insert("sess-1".to_owned(), "aaaaaaaa".to_owned());
        let pump =
            PtyWatchPump::new(parts.views.clone(), parts.roster.clone(), parts.shutdown.clone());
        let handle = tokio::spawn(pump.run(rx));

        tx.send(("sess-1".to_owned(), true)).await.unwrap();
        wait_for_watching(&parts.views, 1).await;

        parts.roster.remove("sess-1");
        tx.send(("sess-1".to_owned(), false)).await.unwrap();
        wait_for_watching(&parts.views, 0).await;

        parts.shutdown.cancel();
        let _ = handle.await;
    }

    /// A driver whose command never returns, standing in for `deliver_reply`'s
    /// 30s settle/confirm loop.
    struct StalledDriver;

    #[async_trait::async_trait]
    impl crate::adapter_runtime::SessionDriver for StalledDriver {
        fn adapter_id(&self) -> &'static str {
            "claude-code"
        }
        async fn spawn(
            &mut self,
            _spec: cctui_proto::adapter::SessionSpec,
            _command_id: Option<uuid::Uuid>,
            _session_id: Option<uuid::Uuid>,
        ) -> crate::adapter_runtime::CommandOutcome {
            std::future::pending().await
        }
        async fn send_message(
            &mut self,
            _local_id: String,
            _text: String,
        ) -> crate::adapter_runtime::CommandOutcome {
            std::future::pending().await
        }
        async fn reply(
            &mut self,
            _local_id: String,
            _text: String,
            _ask_picks: Option<Vec<Vec<usize>>>,
            _env: std::collections::BTreeMap<String, String>,
            _command_id: Option<uuid::Uuid>,
            _turn_id: Option<uuid::Uuid>,
        ) -> crate::adapter_runtime::CommandOutcome {
            std::future::pending().await
        }
        async fn kill(
            &mut self,
            _local_id: String,
            _signal: Option<i32>,
        ) -> crate::adapter_runtime::CommandOutcome {
            std::future::pending().await
        }
    }

    /// The whole point of the separate channel: a `WatchPty` is served while the
    /// serial command loop is still stuck inside a reply's settle loop.
    #[tokio::test]
    async fn a_watch_is_served_while_the_command_loop_is_stalled() {
        let parts = pump_parts();
        parts.roster.insert("sess-1".to_owned(), "aaaaaaaa".to_owned());

        let (cmd_tx, mut cmd_rx) = mpsc::channel(4);
        let (cmd_events, _cmd_events_rx) = mpsc::channel(8);
        let loop_shutdown = parts.shutdown.clone();
        let command_loop = tokio::spawn(async move {
            let mut driver = StalledDriver;
            crate::adapter_runtime::run_command_loop(
                &mut driver,
                &mut cmd_rx,
                &cmd_events,
                &loop_shutdown,
            )
            .await;
        });
        cmd_tx
            .send(cctui_proto::adapter::AdapterCommand::Reply {
                local_id: "sess-1".to_owned(),
                text: "hi".to_owned(),
                ask_picks: None,
                env: std::collections::BTreeMap::new(),
                command_id: None,
                turn_id: None,
            })
            .await
            .unwrap();

        let (tx, rx) = mpsc::channel(4);
        let pump =
            PtyWatchPump::new(parts.views.clone(), parts.roster.clone(), parts.shutdown.clone());
        let pump_handle = tokio::spawn(pump.run(rx));

        tx.send(("sess-1".to_owned(), true)).await.unwrap();
        let started = Instant::now();
        wait_for_watching(&parts.views, 1).await;
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "a watch must not wait on the command loop",
        );
        assert!(!command_loop.is_finished(), "the command must still be stalled");

        parts.shutdown.cancel();
        command_loop.abort();
        let _ = pump_handle.await;
    }

    /// With the event channel full, the post-attach repaint still gets through:
    /// it uses a bounded `send`, while a steady-state frame is dropped.
    #[tokio::test]
    async fn the_first_frame_survives_a_full_event_channel() {
        let dir = tempfile::tempdir().unwrap();
        let (events, mut rx) = mpsc::channel(1);
        events
            .send(AdapterEvent::PtyChunk { local_id: "other".to_owned(), data: String::new() })
            .await
            .unwrap();
        let task = PtyViewTask {
            events,
            discovery: Discovery::with_base(dir.path().to_path_buf()),
            short: "aaaaaaaa".to_owned(),
            local_id: "sess-1".to_owned(),
            cancel: CancellationToken::new(),
            requested_at: Instant::now(),
        };

        // A steady-state frame onto the full channel is dropped outright.
        assert!(task.emit(b"steady", false).await);
        // The first frame waits for the slot the drain below frees.
        let drain = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let first = rx.recv().await;
            let second = rx.recv().await;
            (first, second)
        });
        assert!(task.emit(b"repaint", true).await, "the first frame must not be dropped");
        let (first, second) = drain.await.unwrap();
        assert!(
            matches!(first, Some(AdapterEvent::PtyChunk { local_id, .. }) if local_id == "other")
        );
        let expected = base64::engine::general_purpose::STANDARD.encode(b"repaint");
        match second {
            Some(AdapterEvent::PtyChunk { local_id, data }) => {
                assert_eq!(local_id, "sess-1");
                assert_eq!(data, expected);
            }
            other => panic!("expected the repaint frame, got {other:?}"),
        }
    }
}
