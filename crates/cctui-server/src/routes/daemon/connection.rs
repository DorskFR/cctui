use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::ws::WebSocket;
use axum::extract::{State, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response;
use axum::response::IntoResponse;
use cctui_proto::chunk::Reassembler;
use cctui_proto::ws::{DaemonFrameDown, DaemonFrameUp};
use futures_util::StreamExt;
use futures_util::stream::SplitStream;
use tokio::sync::mpsc;
use uuid::Uuid;

use super::bumps::Bumps;
use super::conn_limit::{ConnGuard, DAEMON_CONNS};
use super::daemon_lost::{PENDING_DAEMON_LOST, schedule_daemon_lost};
use super::decode::{MAX_TRANSFER_BYTES, STALE_TRANSFER};
use super::frames::process_frame;
use super::ingest::{Ingest, ingest_run};
use super::ownership::{SessionOwners, admit, claim_announced};
use super::reconcile::{load_reconcile, load_resume_marks, load_scrub_config};
use crate::state::AppState;
pub use cctui_server_core::targets::daemon::transport::*;

const BUMP_FLUSH: Duration = Duration::from_secs(1);

/// Interrupts are coalesced upstream and drained first, so a handful is plenty.
const PRIORITY_CAPACITY: usize = 16;

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
    let cap = state.config.max_daemon_ws_conns;
    let shared = dispatch_machine(&state, machine_id).await;
    let guard = if shared {
        Some(ConnGuard::exempt())
    } else {
        DAEMON_CONNS.acquire(ctx.key_id, cap, std::time::Instant::now())
    };
    let Some(guard) = guard else {
        tracing::warn!(
            %machine_id,
            cap,
            "daemon WS refused: machine already holds the maximum concurrent connections",
        );
        return Err(StatusCode::TOO_MANY_REQUESTS);
    };
    Ok(ws
        .on_upgrade(move |socket| handle(socket, state, machine_id, user_id, shared, guard))
        .into_response())
}

/// Every dispatched worker pod authenticates against the user's single
/// `dispatch` machine, so a whole fleet shares one machine id and — on the
/// pre-ephemeral-key path — one key. A per-key cap would refuse them, so they
/// stay uncapped; the dispatcher bounds how many pods exist.
async fn dispatch_machine(state: &AppState, machine_id: Uuid) -> bool {
    sqlx::query_scalar::<_, String>("SELECT kind FROM machines WHERE id = $1")
        .bind(machine_id)
        .fetch_optional(&state.pool)
        .await
        .ok()
        .flatten()
        .is_some_and(|kind| kind == "dispatch")
}

async fn handle(
    socket: WebSocket,
    state: AppState,
    machine_id: Uuid,
    user_id: Uuid,
    shared: bool,
    guard: ConnGuard,
) {
    let (sink, mut stream) = socket.split();
    let (tx, rx) = mpsc::channel::<DaemonFrameDown>(64);
    let (priority_tx, priority_rx) = mpsc::channel::<DaemonFrameDown>(PRIORITY_CAPACITY);
    let mut conn = Conn::new(state, machine_id, user_id, shared);
    conn.register(&tx, priority_tx).await;
    send_initial_frames(&conn.state, machine_id, &tx).await;
    let outbound = tokio::spawn(outbound_pump(sink, rx, priority_rx));
    let flusher = spawn_bump_flusher(&conn.bumps, &conn.state.pool);
    tokio::select! {
        () = conn.read_loop(&mut stream, &tx) => {}
        () = guard.evicted() => {
            tracing::warn!(
                %machine_id,
                "daemon WS displaced by a newer connection from the same machine",
            );
        }
    }
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
    /// The machine id is the user's `dispatch` machine, which every dispatched
    /// worker pod authenticates as.
    shared: bool,
    /// Sessions this connection announced. Several daemons can share one
    /// machine id (every dispatched worker pod authenticates as the user's
    /// `dispatch` machine), so the close path may only end these, never the
    /// machine's whole roster.
    announced: Arc<Mutex<HashSet<String>>>,
    owners: SessionOwners,
    bumps: Arc<Bumps>,
}

impl Conn {
    fn new(state: AppState, machine_id: Uuid, user_id: Uuid, shared: bool) -> Self {
        Self {
            state,
            machine_id,
            user_id,
            id: Uuid::new_v4(),
            shared,
            announced: Arc::default(),
            owners: SessionOwners::new(machine_id, user_id),
            bumps: Arc::new(Bumps::default()),
        }
    }

    async fn register(
        &self,
        tx: &mpsc::Sender<DaemonFrameDown>,
        priority_tx: mpsc::Sender<DaemonFrameDown>,
    ) {
        let (state, machine_id) = (&self.state, self.machine_id);
        // Register the daemon for command fan-out with the bus. If a
        // stale entry exists, overwrite it (newest connection wins).
        // A replacement or downgraded daemon must advertise support again.
        state.auth_recovery_daemons.remove(&machine_id);
        state.bus.register_daemon(machine_id, self.id, tx.clone());
        state.bus.register_daemon_priority(self.id, priority_tx);
        if !self.shared {
            PENDING_DAEMON_LOST.cancel_machine(machine_id);
        }
        // Replica-aware presence: record this pod as the WS owner so a
        // peer replica can forward daemon-targeted requests here.
        crate::presence::register(state, crate::presence::Kind::Daemon, machine_id).await;
        report_connect_flap(state, machine_id).await;
        crate::events::record(
            state,
            crate::events::Event::new(
                crate::events::kind::MACHINE_DAEMON_CONNECTED,
                crate::events::Actor::Daemon,
            )
            .machine(machine_id)
            .user(self.user_id)
            .detail(serde_json::json!({ "connection_id": self.id })),
        );
    }

    async fn read_loop(
        &mut self,
        stream: &mut SplitStream<WebSocket>,
        tx: &mpsc::Sender<DaemonFrameDown>,
    ) {
        let mut reader =
            InboundReader::new(stream, DAEMON_READ_TIMEOUT, DAEMON_LIVENESS_CHECK).await;
        let mut reasm = Reassembler::new(MAX_TRANSFER_BYTES);
        loop {
            reasm.evict_older_than(STALE_TRANSFER);
            let inbound = match reader.next().await {
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
                if first {
                    PENDING_DAEMON_LOST.announced(&local_id);
                }
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
        let newest = state.bus.unregister_daemon(machine_id, self.id, tx);
        if newest {
            state.auth_recovery_daemons.remove(&machine_id);
            state.preview.detach_machine(&state.pool, machine_id).await;
            crate::presence::unregister(state, crate::presence::Kind::Daemon, machine_id).await;
            crate::events::record(
                state,
                crate::events::Event::new(
                    crate::events::kind::MACHINE_DAEMON_DISCONNECTED,
                    crate::events::Actor::Daemon,
                )
                .severity(crate::events::Severity::Warn)
                .machine(machine_id)
                .user(self.user_id)
                .detail(serde_json::json!({
                    "connection_id": self.id,
                    "announced_sessions": sessions.len(),
                })),
            );
        }
        // A shared machine's other connections are siblings, not this daemon's
        // replacement, so its sessions are still owed a mark.
        if newest || self.shared {
            schedule_daemon_lost(state, machine_id, self.id, self.shared, sessions);
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
