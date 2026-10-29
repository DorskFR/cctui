//! Shared daemon WebSocket framing, liveness and priority delivery.
use super::decode::{
    decode_binary_frame, decode_compressed_frame, expand_batch, handle_chunk, parse_frame,
};
use axum::extract::ws::Message;
use cctui_proto::adapter::AdapterEvent;
use cctui_proto::chunk::Reassembler;
use cctui_proto::ws::{DaemonFrameDown, DaemonFrameUp};
use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use tokio::sync::mpsc;

/// Evict a daemon whose WS yields no frame of any kind — data, ping, or pong —
///
/// within this window. Measured by frame arrival, not data-message completion,
/// so a slow peer still answering pings mid-transfer is not evicted; a
/// truly half-open one leaves no dead entry in the bus registry.
pub const DAEMON_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_mins(1);
pub const DAEMON_LIVENESS_CHECK: std::time::Duration = std::time::Duration::from_secs(10);

pub fn bearer_token(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_string)
}

pub enum Inbound {
    Data(String),
    Binary(Vec<u8>),
    Skip,
    Done,
    Idle,
}

/// The inbound half of a daemon WS plus its liveness clock.
///
/// The read loop is serial: a decoded frame is processed (often against the
/// database) before the next one is read, so while the server is slow the
/// daemon's frames wait in the socket buffer. Peer silence is therefore only
/// measured while we are actually listening — the clock restarts when the loop
/// comes back for the next frame — and a frame already buffered always wins
/// over the liveness tick. A slow database never evicts a live daemon; a
/// half-open one is still evicted one timeout after we resume listening.
pub struct InboundReader<'a, S> {
    stream: &'a mut S,
    last_frame: tokio::time::Instant,
    liveness: tokio::time::Interval,
    timeout: Duration,
    /// A data frame was handed to the caller, so the time until the next call
    /// was spent processing it, not waiting on the peer.
    handed_out: bool,
}

impl<'a, S> InboundReader<'a, S>
where
    S: futures_util::Stream<Item = Result<Message, axum::Error>> + Unpin,
{
    pub async fn new(stream: &'a mut S, timeout: Duration, check: Duration) -> Self {
        let mut liveness = tokio::time::interval(check);
        liveness.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        liveness.tick().await;
        Self {
            stream,
            last_frame: tokio::time::Instant::now(),
            liveness,
            timeout,
            handed_out: false,
        }
    }

    /// One poll of the inbound WS: the next frame, or a liveness-ticker tick.
    /// Any yielded frame — including a ping/pong tungstenite surfaces mid
    /// data-message — refreshes the clock, so liveness tracks frame arrival
    /// rather than data-message completion. `stream.next()` is cancel-safe, so
    /// dropping it on a ticker tick loses nothing.
    pub async fn next(&mut self) -> Inbound {
        if std::mem::take(&mut self.handed_out) {
            self.last_frame = tokio::time::Instant::now();
        }
        tokio::select! {
            biased;
            item = self.stream.next() => match item {
                Some(Ok(msg)) => {
                    self.last_frame = tokio::time::Instant::now();
                    match msg {
                        Message::Text(t) => {
                            self.handed_out = true;
                            Inbound::Data(t.to_string())
                        }
                        Message::Binary(b) => {
                            self.handed_out = true;
                            Inbound::Binary(b.to_vec())
                        }
                        Message::Close(_) => Inbound::Done,
                        _ => Inbound::Skip,
                    }
                }
                Some(Err(_)) | None => Inbound::Done,
            },
            _ = self.liveness.tick() => {
                if self.last_frame.elapsed() >= self.timeout { Inbound::Idle } else { Inbound::Skip }
            }
        }
    }
}

/// A decoded inbound frame, flattened to the leaf frames to process.
pub enum Leaves {
    Frames(Vec<DaemonFrameUp>),
    Dropped,
    Closed,
}

/// Outbound pump. Besides forwarding `DaemonFrameDown` frames, it sends a
///
/// periodic WS Ping so the daemon always hears from us within its liveness
/// window. Without this, an idle connection (no commands queued) sends the
/// daemon nothing after the initial Reconcile — axum does not auto-flush a
/// Pong on the split sink while it's otherwise idle — so the daemon's
/// half-open detector tears the WS down every 60s and flaps forever.
/// The interval mirrors the daemon's 20s ping cadence and stays
/// well under both sides' 60s timeouts.
pub async fn outbound_pump<S>(
    mut sink: S,
    mut rx: mpsc::Receiver<DaemonFrameDown>,
    mut priority: mpsc::Receiver<DaemonFrameDown>,
) where
    S: futures_util::Sink<Message> + Unpin,
{
    let mut keepalive = tokio::time::interval(std::time::Duration::from_secs(20));
    keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    keepalive.tick().await; // discard the immediate first tick
    let mut priority_open = true;
    loop {
        tokio::select! {
            // Biased so an interrupt never waits behind the ordinary queue.
            biased;
            frame = priority.recv(), if priority_open => {
                let Some(frame) = frame else {
                    priority_open = false;
                    continue;
                };
                let Ok(json) = serde_json::to_string(&frame) else { continue };
                if sink.send(Message::Text(json.into())).await.is_err() {
                    break;
                }
            }
            frame = rx.recv() => {
                let Some(frame) = frame else { break };
                let Ok(json) = serde_json::to_string(&frame) else { continue };
                if sink.send(Message::Text(json.into())).await.is_err() {
                    break;
                }
            }
            _ = keepalive.tick() => {
                if sink.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break;
                }
            }
        }
    }
}

#[must_use]
pub fn decode_inbound(inbound: Inbound) -> Option<DaemonFrameUp> {
    match inbound {
        Inbound::Data(text) => parse_frame(text.as_bytes()),
        Inbound::Binary(data) => decode_binary_frame(&data)
            .map(|inner| DaemonFrameUp::Batch { frames: expand_batch(inner) }),
        Inbound::Skip | Inbound::Done | Inbound::Idle => None,
    }
}

/// Unwrap a chunk, compressed or batch envelope into its leaf frames. A chunk
/// is acked first; `Closed` means the ack could not be sent.
pub async fn leaves_of(
    frame: DaemonFrameUp,
    reasm: &mut Reassembler,
    tx: &mpsc::Sender<DaemonFrameDown>,
) -> Leaves {
    match frame {
        DaemonFrameUp::Chunk { transfer_id, chunk_index, total_chunks, data, codec } => {
            let (ack, inner) = handle_chunk(
                reasm,
                transfer_id,
                chunk_index,
                total_chunks,
                &data,
                codec.as_deref(),
            );
            if tx.send(ack).await.is_err() {
                return Leaves::Closed;
            }
            inner.map_or(Leaves::Dropped, |inner| Leaves::Frames(expand_batch(inner)))
        }
        DaemonFrameUp::Compressed { codec, data } => decode_compressed_frame(&codec, &data)
            .map_or(Leaves::Dropped, |inner| Leaves::Frames(expand_batch(inner))),
        DaemonFrameUp::Batch { frames } => Leaves::Frames(frames),
        other => Leaves::Frames(vec![other]),
    }
}

/// The session a daemon frame announces as live on this connection, if any.
#[must_use]
pub fn announced_session(frame: &DaemonFrameUp) -> Option<&str> {
    match frame {
        DaemonFrameUp::SessionRegistered { local_id, .. }
        | DaemonFrameUp::Event { event: AdapterEvent::SessionStarted { local_id, .. }, .. } => {
            Some(local_id)
        }
        _ => None,
    }
}

/// The session a daemon frame acts on, if any.
#[must_use]
pub fn session_scope(frame: &DaemonFrameUp) -> Option<&str> {
    match frame {
        DaemonFrameUp::SessionRegistered { local_id, .. }
        | DaemonFrameUp::Event {
            event:
                AdapterEvent::SessionStarted { local_id, .. }
                | AdapterEvent::Message { local_id, .. }
                | AdapterEvent::ToolUse { local_id, .. }
                | AdapterEvent::SessionEnded { local_id, .. }
                | AdapterEvent::Status { local_id, .. }
                | AdapterEvent::PrLink { local_id, .. }
                | AdapterEvent::TokenUsage { local_id, .. }
                | AdapterEvent::SessionModel { local_id, .. }
                | AdapterEvent::PermissionRequest { local_id, .. }
                | AdapterEvent::PermissionResolved { local_id, .. }
                | AdapterEvent::AskQuestion { local_id, .. }
                | AdapterEvent::AskResolved { local_id }
                | AdapterEvent::PlanRequest { local_id, .. }
                | AdapterEvent::PlanResolved { local_id }
                | AdapterEvent::Diagnose { local_id, .. }
                | AdapterEvent::PtyChunk { local_id, .. }
                | AdapterEvent::TextDelta { local_id, .. }
                | AdapterEvent::GuardStepState { local_id, .. }
                | AdapterEvent::TranscriptMark { local_id, .. }
                | AdapterEvent::TurnEnd { local_id, .. }
                | AdapterEvent::RateLimits { local_id, .. }
                | AdapterEvent::Todos { local_id, .. }
                | AdapterEvent::Title { local_id, .. },
            ..
        } => Some(local_id),
        _ => None,
    }
}

#[must_use]
pub fn frame_trace(frame: &DaemonFrameUp) -> String {
    match frame {
        DaemonFrameUp::SessionRegistered { adapter_id, local_id } => {
            format!("session_registered adapter={adapter_id} local_id={local_id}")
        }
        DaemonFrameUp::Event { adapter_id, event } => {
            format!(
                "event adapter={adapter_id} kind={} local_id={}",
                event_kind(event),
                event_local_id(event),
            )
        }
        DaemonFrameUp::Heartbeat { .. } => "heartbeat".to_owned(),
        _ => "other".to_owned(),
    }
}

#[must_use]
pub fn event_local_id(event: &AdapterEvent) -> &str {
    match event {
        AdapterEvent::SessionStarted { local_id, .. }
        | AdapterEvent::Message { local_id, .. }
        | AdapterEvent::ToolUse { local_id, .. }
        | AdapterEvent::SessionEnded { local_id, .. }
        | AdapterEvent::Status { local_id, .. }
        | AdapterEvent::PermissionRequest { local_id, .. }
        | AdapterEvent::PermissionResolved { local_id, .. }
        | AdapterEvent::TokenUsage { local_id, .. }
        | AdapterEvent::TranscriptMark { local_id, .. }
        | AdapterEvent::GuardStepState { local_id, .. }
        | AdapterEvent::TurnEnd { local_id, .. }
        | AdapterEvent::RateLimits { local_id, .. }
        | AdapterEvent::Todos { local_id, .. }
        | AdapterEvent::Title { local_id, .. } => local_id,
        _ => "",
    }
}

#[must_use]
pub const fn event_kind(event: &AdapterEvent) -> &'static str {
    match event {
        AdapterEvent::SessionStarted { .. } => "session_started",
        AdapterEvent::Message { .. } => "message",
        AdapterEvent::ToolUse { .. } => "tool_use",
        AdapterEvent::SessionEnded { .. } => "session_ended",
        AdapterEvent::Status { .. } => "status",
        AdapterEvent::TokenUsage { .. } => "token_usage",
        AdapterEvent::PermissionRequest { .. } => "permission_request",
        AdapterEvent::PermissionResolved { .. } => "permission_resolved",
        AdapterEvent::TranscriptMark { .. } => "transcript_mark",
        AdapterEvent::TextDelta { .. } => "text_delta",
        AdapterEvent::GuardStepState { .. } => "guard_step_state",
        AdapterEvent::TurnEnd { .. } => "turn_end",
        AdapterEvent::Todos { .. } => "todos",
        AdapterEvent::Title { .. } => "title",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use cctui_proto::adapter::EndReason;
    use futures_util::Stream;

    use super::*;

    async fn drive<S>(mut stream: S, timeout: Duration, check: Duration) -> Inbound
    where
        S: Stream<Item = Result<Message, axum::Error>> + Unpin,
    {
        let mut reader = InboundReader::new(&mut stream, timeout, check).await;
        loop {
            match reader.next().await {
                Inbound::Data(_) | Inbound::Binary(_) | Inbound::Skip => {}
                term @ (Inbound::Done | Inbound::Idle) => return term,
            }
        }
    }

    #[allow(clippy::unnecessary_wraps)] // the item type of the WS stream
    fn text(s: &str) -> Result<Message, axum::Error> {
        Ok(Message::Text(s.to_owned().into()))
    }

    /// The production loop processes each frame before reading the next. A
    /// database stall longer than the read timeout leaves the daemon's next
    /// frames buffered; they must be read, not lost to an eviction.
    #[tokio::test(start_paused = true)]
    async fn processing_slower_than_the_read_timeout_never_evicts_a_live_daemon() {
        let stall = DAEMON_READ_TIMEOUT + Duration::from_secs(30);
        let mut stream = futures_util::stream::iter([text("a"), text("b"), text("c")]);
        let mut reader =
            InboundReader::new(&mut stream, DAEMON_READ_TIMEOUT, DAEMON_LIVENESS_CHECK).await;
        let mut seen = Vec::new();
        loop {
            match reader.next().await {
                Inbound::Data(t) => {
                    seen.push(t);
                    tokio::time::sleep(stall).await;
                }
                Inbound::Skip => {}
                Inbound::Idle => panic!("live daemon evicted after {seen:?}"),
                Inbound::Binary(_) | Inbound::Done => break,
            }
        }
        assert_eq!(seen, ["a", "b", "c"]);
    }

    /// The stall does not hide a real disconnect: once the loop listens again,
    /// a silent peer is evicted one read timeout later, not sooner.
    #[tokio::test(start_paused = true)]
    async fn a_silent_peer_is_evicted_one_timeout_after_a_slow_frame() {
        let stall = DAEMON_READ_TIMEOUT * 2;
        let mut stream = futures_util::stream::iter([text("a")])
            .chain(futures_util::stream::pending::<Result<Message, axum::Error>>());
        let mut reader =
            InboundReader::new(&mut stream, DAEMON_READ_TIMEOUT, DAEMON_LIVENESS_CHECK).await;
        assert!(matches!(reader.next().await, Inbound::Data(ref t) if t == "a"));
        tokio::time::sleep(stall).await;
        let resumed = tokio::time::Instant::now();
        let out = loop {
            match reader.next().await {
                Inbound::Skip => {}
                other => break other,
            }
        };
        assert!(matches!(out, Inbound::Idle));
        let silent = resumed.elapsed();
        assert!(silent >= DAEMON_READ_TIMEOUT, "evicted after only {silent:?}");
        assert!(silent <= DAEMON_READ_TIMEOUT + DAEMON_LIVENESS_CHECK, "{silent:?}");
    }

    fn command(command: cctui_proto::adapter::AdapterCommand) -> DaemonFrameDown {
        DaemonFrameDown::Command { adapter_id: "codex".into(), command: Box::new(command) }
    }

    #[tokio::test]
    async fn the_pump_sends_a_priority_frame_before_the_queued_ones() {
        use cctui_proto::adapter::AdapterCommand;
        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Message>();
        let sink = Box::pin(futures_util::sink::unfold(out_tx, |tx, msg: Message| async move {
            tx.send(msg).map_err(|_| ())?;
            Ok::<_, ()>(tx)
        }));
        let (tx, rx) = mpsc::channel(8);
        let (priority_tx, priority_rx) = mpsc::channel(8);
        for name in ["a", "b", "c"] {
            tx.try_send(command(AdapterCommand::Rename {
                local_id: "s1".into(),
                name: name.into(),
            }))
            .unwrap();
        }
        priority_tx
            .try_send(command(AdapterCommand::Interrupt {
                local_id: "s1".into(),
                command_id: None,
            }))
            .unwrap();
        let pump = tokio::spawn(outbound_pump(sink, rx, priority_rx));
        let first = tokio::time::timeout(Duration::from_secs(1), out_rx.recv()).await.unwrap();
        let Some(Message::Text(text)) = first else { panic!("expected a text frame") };
        assert!(text.as_str().contains("interrupt"), "{}", text.as_str());
        for _ in 0..3 {
            let next = tokio::time::timeout(Duration::from_secs(1), out_rx.recv()).await.unwrap();
            assert!(matches!(next, Some(Message::Text(t)) if t.as_str().contains("rename")));
        }
        drop(tx);
        drop(priority_tx);
        tokio::time::timeout(Duration::from_secs(1), pump).await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn half_open_peer_is_evicted() {
        let stream = futures_util::stream::pending::<Result<Message, axum::Error>>();
        let out = drive(stream, Duration::from_millis(300), Duration::from_millis(25)).await;
        assert!(matches!(out, Inbound::Idle));
    }

    #[tokio::test]
    async fn slow_trickle_answering_pings_survives() {
        let stream = futures_util::stream::unfold(0u32, |i| async move {
            if i >= 12 {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
            Some((Ok::<_, axum::Error>(Message::Pong(Vec::new().into())), i + 1))
        });
        let stream = Box::pin(stream);
        let out = drive(stream, Duration::from_millis(300), Duration::from_millis(25)).await;
        assert!(matches!(out, Inbound::Done));
    }

    #[test]
    fn bearer_token_reads_authorization_header() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            axum::http::HeaderValue::from_static("Bearer mkey-123"),
        );
        assert_eq!(bearer_token(&headers).as_deref(), Some("mkey-123"));
        assert!(bearer_token(&axum::http::HeaderMap::new()).is_none());
    }

    #[test]
    fn ws_auth_is_header_only() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            axum::http::HeaderValue::from_static("Bearer header-key"),
        );
        assert_eq!(bearer_token(&headers).as_deref(), Some("header-key"));
        assert!(bearer_token(&axum::http::HeaderMap::new()).is_none());
    }

    #[test]
    fn transcript_mark_event_is_recognized_and_routed() {
        let ev = cctui_proto::adapter::AdapterEvent::TranscriptMark {
            local_id: "sess-9".into(),
            offset: 4096,
        };
        assert_eq!(event_kind(&ev), "transcript_mark");
        assert_eq!(event_local_id(&ev), "sess-9");
    }

    #[tokio::test]
    async fn a_binary_message_is_surfaced_as_binary() {
        let mut stream = futures_util::stream::iter([Ok::<_, axum::Error>(Message::Binary(
            vec![1_u8, 2, 3].into(),
        ))]);
        let mut reader =
            InboundReader::new(&mut stream, Duration::from_hours(1), Duration::from_hours(1)).await;
        let out = reader.next().await;
        assert!(matches!(out, Inbound::Binary(ref b) if b == &[1, 2, 3]));
    }

    #[test]
    fn announced_session_reads_registration_and_start_frames() {
        use cctui_proto::adapter::{AdapterEvent, SessionMeta};
        let registered = DaemonFrameUp::SessionRegistered {
            adapter_id: "claude-code".into(),
            local_id: "s1".into(),
        };
        let started = DaemonFrameUp::Event {
            adapter_id: "codex".into(),
            event: AdapterEvent::SessionStarted {
                local_id: "s2".into(),
                meta: SessionMeta::default(),
            },
        };
        let other = DaemonFrameUp::Event {
            adapter_id: "codex".into(),
            event: AdapterEvent::SessionEnded {
                local_id: "s3".into(),
                reason: EndReason::Completed,
            },
        };
        assert_eq!(super::announced_session(&registered), Some("s1"));
        assert_eq!(super::announced_session(&started), Some("s2"));
        assert_eq!(super::announced_session(&other), None);
    }
}
