use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{State, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response;
use axum::response::IntoResponse;
use cctui_proto::adapter::AdapterEvent;
use cctui_proto::chunk::Reassembler;
use cctui_proto::ws::{DaemonFrameDown, DaemonFrameUp};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use uuid::Uuid;

use super::bumps::Bumps;
use super::daemon_lost::{PENDING_DAEMON_LOST, schedule_daemon_lost};
use super::decode::{
    MAX_TRANSFER_BYTES, STALE_TRANSFER, decode_binary_frame, decode_compressed_frame, expand_batch,
    handle_chunk,
};
use super::frames::process_frame;
use super::ingest::{Ingest, ingest_run};
use super::ownership::{SessionOwners, admit, claim_announced};
use super::reconcile::{load_reconcile, load_resume_marks, load_scrub_config};
use crate::state::AppState;

/// Evict a daemon whose WS yields no frame of any kind — data, ping, or pong —
/// within this window. Measured by frame arrival, not data-message completion,
/// so a slow peer still answering pings mid-transfer is not evicted; a
/// truly half-open one leaves no dead entry in the bus registry.
const DAEMON_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_mins(1);
const DAEMON_LIVENESS_CHECK: std::time::Duration = std::time::Duration::from_secs(10);

const BUMP_FLUSH: Duration = Duration::from_secs(1);

// ---- /api/v1/daemon/ws ----

pub async fn ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<response::Response, StatusCode> {
    let token = bearer_token(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let ctx = state.auth_config.validate(&token).await.ok_or(StatusCode::UNAUTHORIZED)?;
    let Some(machine_id) = ctx.machine_id else {
        return Err(StatusCode::FORBIDDEN);
    };
    let user_id = ctx.user_id;
    Ok(ws.on_upgrade(move |socket| handle(socket, state, machine_id, user_id)).into_response())
}

fn bearer_token(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_string)
}

enum Inbound {
    Data(String),
    Binary(Vec<u8>),
    Skip,
    Done,
    Idle,
}

/// One poll of the inbound WS: the next frame, or a liveness-ticker tick. Any
/// yielded frame — including a ping/pong tungstenite surfaces mid data-message —
/// refreshes `last_frame`, so liveness tracks frame arrival rather than
/// data-message completion. `stream.next()` is cancel-safe, so dropping
/// it on a ticker tick loses nothing.
async fn next_inbound<S>(
    stream: &mut S,
    last_frame: &mut tokio::time::Instant,
    liveness: &mut tokio::time::Interval,
    timeout: std::time::Duration,
) -> Inbound
where
    S: futures_util::Stream<Item = Result<Message, axum::Error>> + Unpin,
{
    tokio::select! {
        item = stream.next() => match item {
            Some(Ok(msg)) => {
                *last_frame = tokio::time::Instant::now();
                match msg {
                    Message::Text(t) => Inbound::Data(t.to_string()),
                    Message::Binary(b) => Inbound::Binary(b.to_vec()),
                    Message::Close(_) => Inbound::Done,
                    _ => Inbound::Skip,
                }
            }
            Some(Err(_)) | None => Inbound::Done,
        },
        _ = liveness.tick() => {
            if last_frame.elapsed() >= timeout { Inbound::Idle } else { Inbound::Skip }
        }
    }
}

async fn handle(socket: WebSocket, state: AppState, machine_id: Uuid, user_id: Uuid) {
    let (sink, mut stream) = socket.split();
    let (tx, rx) = mpsc::channel::<DaemonFrameDown>(64);
    let mut conn = Conn::new(state, machine_id, user_id);
    conn.register(&tx).await;
    send_initial_frames(&conn.state, machine_id, &tx).await;
    let outbound = tokio::spawn(outbound_pump(sink, rx));
    let flusher = spawn_bump_flusher(&conn.bumps, &conn.state.pool);
    conn.read_loop(&mut stream, &tx).await;
    conn.close(&tx).await;
    outbound.abort();
    flusher.abort();
    conn.bumps.flush(&conn.state.pool).await;
}

/// One daemon WS connection.
struct Conn {
    state: AppState,
    machine_id: Uuid,
    user_id: Uuid,
    /// This connection's own routing address. The machine id groups the
    /// dispatched pods; only this distinguishes them.
    id: Uuid,
    /// Sessions this connection announced. Several daemons can share one
    /// machine id (every dispatched worker pod authenticates as the user's
    /// `dispatch` machine), so the close path may only end these, never the
    /// machine's whole roster.
    announced: Arc<Mutex<HashSet<String>>>,
    owners: SessionOwners,
    bumps: Arc<Bumps>,
}

/// A decoded inbound frame, flattened to the leaf frames to process.
enum Leaves {
    Frames(Vec<DaemonFrameUp>),
    Dropped,
    Closed,
}

impl Conn {
    fn new(state: AppState, machine_id: Uuid, user_id: Uuid) -> Self {
        Self {
            state,
            machine_id,
            user_id,
            id: Uuid::new_v4(),
            announced: Arc::default(),
            owners: SessionOwners::new(machine_id, user_id),
            bumps: Arc::new(Bumps::default()),
        }
    }

    async fn register(&self, tx: &mpsc::Sender<DaemonFrameDown>) {
        let (state, machine_id) = (&self.state, self.machine_id);
        // Register the daemon for command fan-out with the bus. If a
        // stale entry exists, overwrite it (newest connection wins).
        state.bus.register_daemon(machine_id, self.id, tx.clone());
        PENDING_DAEMON_LOST.cancel(machine_id);
        // Replica-aware presence: record this pod as the WS owner so a
        // peer replica can forward daemon-targeted requests here.
        crate::presence::register(state, crate::presence::Kind::Daemon, machine_id).await;
        report_connect_flap(state, machine_id).await;
    }

    async fn read_loop(
        &mut self,
        stream: &mut SplitStream<WebSocket>,
        tx: &mpsc::Sender<DaemonFrameDown>,
    ) {
        let mut last_frame = tokio::time::Instant::now();
        let mut liveness = tokio::time::interval(DAEMON_LIVENESS_CHECK);
        liveness.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        liveness.tick().await;
        let mut reasm = Reassembler::new(MAX_TRANSFER_BYTES);
        loop {
            reasm.evict_older_than(STALE_TRANSFER);
            let inbound =
                match next_inbound(stream, &mut last_frame, &mut liveness, DAEMON_READ_TIMEOUT)
                    .await
                {
                    data @ (Inbound::Data(_) | Inbound::Binary(_)) => data,
                    Inbound::Skip => continue,
                    Inbound::Done => break,
                    Inbound::Idle => {
                        note_idle_eviction(&self.state, self.machine_id);
                        break;
                    }
                };
            let Some(frame) = decode_inbound(inbound) else { continue };
            match leaves_of(frame, &mut reasm, tx).await {
                Leaves::Frames(leaves) => self.ingest_leaves(leaves).await,
                Leaves::Dropped => {}
                Leaves::Closed => break,
            }
        }
    }

    async fn ingest_leaves(&mut self, leaves: Vec<DaemonFrameUp>) {
        let (machine_id, user_id, conn_id) = (self.machine_id, self.user_id, self.id);
        let (state, bumps) = (&self.state, &self.bumps);
        let mut run: Vec<Ingest> = Vec::new();
        for frame in leaves {
            if let Some(local_id) = session_scope(&frame)
                && !admit(&mut self.owners, &state.pool, local_id).await
            {
                continue;
            }
            let frame = match Ingest::take(frame) {
                Ok(ingest) => {
                    run.push(ingest);
                    continue;
                }
                Err(frame) => *frame,
            };
            ingest_run(state, bumps, machine_id, user_id, &mut run).await;
            let announce = announced_session(&frame).map(str::to_owned);
            if let Some(local_id) = &announce {
                state.bus.bind_session_conn(local_id, conn_id);
            }
            let trace = frame_trace(&frame);
            if let Err(err) = process_frame(state, bumps, machine_id, user_id, frame).await {
                tracing::warn!(%err, %trace, "process_frame error");
            }
            if let Some(local_id) = announce
                && claim_announced(&mut self.owners, &state.pool, &state.bus, conn_id, &local_id)
                    .await
            {
                // First announcement only: these frames repeat constantly and
                // the presence row is a DB upsert.
                let first = self.announced.lock().is_ok_and(|mut set| set.insert(local_id.clone()));
                if first && let Ok(session) = Uuid::parse_str(&local_id) {
                    crate::presence::register(state, crate::presence::Kind::Session, session).await;
                }
            }
        }
        ingest_run(state, bumps, machine_id, user_id, &mut run).await;
    }

    /// Cleanup. Only drop the entry if it is STILL OURS. During a reconnect
    /// race the daemon's new connection may have already overwritten the bus
    /// registry with its own `tx` ("newest wins" in [`Conn::register`]); an
    /// unconditional remove would delete that live channel, so every command
    /// would silently fail `NoDaemon` while events kept flowing (they go through
    /// `process_frame`, which never touches the connection registry). The
    /// bus's `unregister_daemon` applies the same-channel guard. The
    /// presence row mirrors it, with its own pod guard for the cross-pod twin
    /// of the same race.
    async fn close(&self, tx: &mpsc::Sender<DaemonFrameDown>) {
        let (state, machine_id) = (&self.state, self.machine_id);
        let sessions: Vec<String> =
            self.announced.lock().map(|mut set| set.drain().collect()).unwrap_or_default();
        // Session rows belong to THIS connection, so they go whether or not the
        // machine entry was still ours.
        for session in sessions.iter().filter_map(|s| Uuid::parse_str(s).ok()) {
            crate::presence::unregister(state, crate::presence::Kind::Session, session).await;
        }
        // Detach, don't close: a rolling restart reconnects the daemon (maybe
        // on another pod) and it re-announces these previews by id.
        for session in &sessions {
            state.preview.detach_session(&state.pool, session).await;
        }
        if state.bus.unregister_daemon(machine_id, self.id, tx) {
            state.preview.detach_machine(&state.pool, machine_id).await;
            crate::presence::unregister(state, crate::presence::Kind::Daemon, machine_id).await;
            schedule_daemon_lost(state, machine_id, sessions);
        }
    }
}

async fn report_connect_flap(state: &AppState, machine_id: Uuid) {
    let Some(flap) = state.connect_tracker.record(machine_id) else {
        return;
    };
    tracing::error!(
        %machine_id,
        connects = flap.connects,
        window_mins = crate::bandwidth_watch::CONNECT_FLAP_WINDOW.as_secs() / 60,
        "daemon WS crashloop suspected — machine reconnecting rapidly",
    );
    if flap.notify {
        let name: String =
            sqlx::query_scalar("SELECT COALESCE(display_name, name) FROM machines WHERE id = $1")
                .bind(machine_id)
                .fetch_optional(&state.pool)
                .await
                .ok()
                .flatten()
                .unwrap_or_else(|| machine_id.to_string());
        crate::ntfy::notify(
            &state.config,
            crate::ntfy::Notification {
                title: format!("Daemon crashloop: {name}"),
                message: format!(
                    "machine {name} ({machine_id}) opened {} daemon WS connections \
                     in the last {} minutes — its daemon is likely crashlooping",
                    flap.connects,
                    crate::bandwidth_watch::CONNECT_FLAP_WINDOW.as_secs() / 60,
                ),
                tags: "rotating_light".into(),
                priority: 4,
            },
        );
    }
}

async fn send_initial_frames(
    state: &AppState,
    machine_id: Uuid,
    tx: &mpsc::Sender<DaemonFrameDown>,
) {
    // Send Reconcile immediately.
    match load_reconcile(state, machine_id).await {
        Ok(adapters) => {
            let secret_scrub = load_scrub_config(state, machine_id).await;
            if tx.send(DaemonFrameDown::Reconcile { adapters, secret_scrub }).await.is_err() {
                tracing::warn!("daemon tx closed before reconcile");
            }
        }
        Err(err) => {
            tracing::error!(%err, "load_reconcile failed");
        }
    }

    // Resume marks must follow Reconcile: the daemon needs its adapters live to
    // route the marks to before it can clamp their tail cursors.
    match load_resume_marks(state, machine_id).await {
        Ok(session_marks) if !session_marks.is_empty() => {
            let frame = DaemonFrameDown::ResumeMarks { session_marks, archived: Vec::new() };
            if tx.send(frame).await.is_err() {
                tracing::warn!("daemon tx closed before resume marks");
            }
        }
        Ok(_) => {}
        Err(err) => tracing::error!(%err, "load_resume_marks failed"),
    }
}

/// Outbound pump. Besides forwarding `DaemonFrameDown` frames, it sends a
/// periodic WS Ping so the daemon always hears from us within its liveness
/// window. Without this, an idle connection (no commands queued) sends the
/// daemon nothing after the initial Reconcile — axum does not auto-flush a
/// Pong on the split sink while it's otherwise idle — so the daemon's
/// half-open detector tears the WS down every 60s and flaps forever.
/// The interval mirrors the daemon's 20s ping cadence and stays
/// well under both sides' 60s timeouts.
async fn outbound_pump(
    mut sink: SplitSink<WebSocket, Message>,
    mut rx: mpsc::Receiver<DaemonFrameDown>,
) {
    let mut keepalive = tokio::time::interval(std::time::Duration::from_secs(20));
    keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    keepalive.tick().await; // discard the immediate first tick
    loop {
        tokio::select! {
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

fn spawn_bump_flusher(bumps: &Arc<Bumps>, pool: &sqlx::PgPool) -> tokio::task::JoinHandle<()> {
    let (bumps, pool) = (Arc::clone(bumps), pool.clone());
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(BUMP_FLUSH);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            bumps.flush(&pool).await;
        }
    })
}

fn note_idle_eviction(state: &AppState, machine_id: Uuid) {
    tracing::warn!(%machine_id, "daemon WS idle past read timeout — evicting");
    let count = state.eviction_tracker.record(machine_id);
    if count >= crate::bandwidth_watch::EVICTION_THRESHOLD {
        tracing::error!(
            %machine_id,
            evictions = count,
            window_mins = crate::bandwidth_watch::EVICTION_WINDOW.as_secs() / 60,
            "daemon WS eviction loop — machine evicted repeatedly; \
             suspected re-upload/re-connect loop",
        );
    }
}

fn decode_inbound(inbound: Inbound) -> Option<DaemonFrameUp> {
    match inbound {
        Inbound::Data(text) => match serde_json::from_str(&text) {
            Ok(f) => Some(f),
            Err(err) => {
                tracing::warn!(%err, "bad daemon frame");
                None
            }
        },
        Inbound::Binary(data) => decode_binary_frame(&data)
            .map(|inner| DaemonFrameUp::Batch { frames: expand_batch(inner) }),
        Inbound::Skip | Inbound::Done | Inbound::Idle => None,
    }
}

/// Unwrap a chunk, compressed or batch envelope into its leaf frames. A chunk
/// is acked first; `Closed` means the ack could not be sent.
async fn leaves_of(
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
fn announced_session(frame: &DaemonFrameUp) -> Option<&str> {
    match frame {
        DaemonFrameUp::SessionRegistered { local_id, .. }
        | DaemonFrameUp::Event { event: AdapterEvent::SessionStarted { local_id, .. }, .. } => {
            Some(local_id)
        }
        _ => None,
    }
}

/// The session a daemon frame acts on, if any.
pub(super) fn session_scope(frame: &DaemonFrameUp) -> Option<&str> {
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
                | AdapterEvent::TranscriptMark { local_id, .. }
                | AdapterEvent::RateLimits { local_id, .. },
            ..
        } => Some(local_id),
        _ => None,
    }
}

fn frame_trace(frame: &DaemonFrameUp) -> String {
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

pub(super) fn event_local_id(event: &AdapterEvent) -> &str {
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
        | AdapterEvent::RateLimits { local_id, .. } => local_id,
        _ => "",
    }
}

pub(super) const fn event_kind(event: &AdapterEvent) -> &'static str {
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
        let mut last = tokio::time::Instant::now();
        let mut liveness = tokio::time::interval(check);
        liveness.tick().await;
        loop {
            match next_inbound(&mut stream, &mut last, &mut liveness, timeout).await {
                Inbound::Data(_) | Inbound::Binary(_) | Inbound::Skip => {}
                term @ (Inbound::Done | Inbound::Idle) => return term,
            }
        }
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
        let mut last = tokio::time::Instant::now();
        let mut liveness = tokio::time::interval(Duration::from_hours(1));
        liveness.tick().await;
        let out =
            next_inbound(&mut stream, &mut last, &mut liveness, Duration::from_hours(1)).await;
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
