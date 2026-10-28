//! `PeerHttpTransport`: cross-replica routing and event fan-out over
//! pod-to-pod HTTP.
//!
//! * **route**: a local registry miss looks up the live owning peer in
//!   `ws_presence` and POSTs the frame to its `/internal/bus/route`; no live
//!   owner yields the same `NoDaemon`/`NoDispatcher` miss as locally. An
//!   owner that cannot be connected to is a replaced pod: its row is dropped
//!   and the request waits briefly for the reconnect to name a new owner.
//! * **relay**: every locally-published [`BusEvent`] is batched and sent to
//!   every live pod's `/internal/bus/publish`, best-effort; the DB stays the
//!   source of truth for refetch.
//!
//! Receiving endpoints deliver locally only, so nothing is re-forwarded. Auth is
//! a shared secret minted once into `cluster_secrets`, compared in constant
//! time; no user or machine token can reach these endpoints.

use cctui_proto::adapter::BootstrapFile;
use cctui_proto::git::GitInfo;
use cctui_proto::ws::{
    AgentEvent, DaemonFrameDown, DispatcherFrameDown, DispatcherFrameUp, ReadFileErrorKind,
    ReadFileOk, ServerEvent,
};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::mpsc;
use uuid::Uuid;

use super::{BusError, BusEvent, DaemonRequest, DaemonResponse, Transport, VoiceEvent};
use crate::presence::{self, Kind};

/// Timeout for a forwarded fire-and-forget command (the peer only has to drop
/// the frame on a local channel).
const COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// Timeout for a forwarded stage-files round-trip: the peer's own in-bus wait
/// is `STAGE_TIMEOUT` (30s), plus headroom for the upload body transfer.
const STAGE_FORWARD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(40);
/// Timeout for a forwarded list-dirs round-trip (peer waits 3s).
const LIST_DIRS_FORWARD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);
/// Timeout for a forwarded session-diagnose round-trip (peer waits 10s).
const DIAGNOSE_FORWARD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
/// Outlasts the bus's 60 s `READ_FILE_TIMEOUT`.
const READ_FILE_FORWARD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(70);
/// Timeout for a forwarded dispatcher round-trip (peer waits 30s).
const DISPATCHER_FORWARD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(35);
/// Timeout for one relay POST to one peer. Publish is best-effort; a slow peer
/// must not back the relay queue up for long.
const RELAY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
/// Max events drained into one relay batch.
const RELAY_BATCH: usize = 64;
/// Capacity of the relay ingress and of each peer's queue. Overflow is
/// dropped: peers recover dropped events from the DB on resubscribe.
const RELAY_QUEUE: usize = 4096;
/// How long a fetched peer-pod list is reused before re-querying `pods`.
const PEER_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(5);
/// Timeout for one auth-cache purge or guard forget POST to one peer.
const AUTH_PURGE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);
/// How long a route waits for a replacement owner after the recorded one was
/// unreachable (the daemon's reconnect to a live pod lands within this), and
/// how often the owner row is re-read meanwhile.
const REOWN_WAIT: std::time::Duration = std::time::Duration::from_secs(3);
const REOWN_POLL: std::time::Duration = std::time::Duration::from_millis(250);

// ---- wire types (shared with `crate::routes::internal`) ----

/// Body of `POST /internal/bus/route`: one frame/round-trip addressed at a WS
/// the receiving pod is believed to hold.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum RouteRequest {
    DaemonCommand {
        machine: Uuid,
        frame: DaemonFrameDown,
    },
    DaemonStageFiles {
        machine: Uuid,
        adapter_id: String,
        local_id: String,
        uploads: Vec<BootstrapFile>,
    },
    DaemonListDirs {
        machine: Uuid,
        path: String,
    },
    DaemonGitInfo {
        machine: Uuid,
        path: String,
        include_dirty: bool,
    },
    DaemonReadFile {
        machine: Uuid,
        path: String,
        max_bytes: u64,
        cwd: Option<String>,
    },
    /// Session-diagnose round-trip: the receiving pod runs the full
    /// command-down / event-up correlation locally and returns the report.
    DaemonDiagnose {
        machine: Uuid,
        adapter_id: String,
        local_id: String,
    },
    DispatcherCommand {
        dispatcher: Uuid,
        frame: DispatcherFrameDown,
    },
    /// Correlated dispatcher round-trip; `request_id` is the caller-minted
    /// correlation id already embedded in `frame`.
    DispatcherRequest {
        dispatcher: Uuid,
        request_id: Uuid,
        frame: DispatcherFrameDown,
    },
}

/// Body of the `POST /internal/bus/route` response. Always HTTP 200 from the
/// handler; delivery failures ride the `Err` variant so [`BusError`] semantics
/// survive the hop.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum RouteResponse {
    Ok,
    StagedFiles {
        paths: Vec<String>,
    },
    Dirs {
        dirs: Vec<String>,
    },
    GitInfo {
        info: GitInfo,
    },
    File {
        file: ReadFileOk,
    },
    Diagnose {
        report: Box<cctui_proto::diagnose::SessionDiagnose>,
    },
    DispatcherReply {
        frame: DispatcherFrameUp,
    },
    Err {
        code: WireErrorCode,
        message: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        allowed_folders: Vec<String>,
    },
}

impl RouteResponse {
    pub fn from_error(err: &BusError) -> Self {
        let (code, message) = encode_error(err);
        let allowed_folders = match err {
            BusError::ReadFile(r) => r.allowed_folders.clone(),
            _ => Vec::new(),
        };
        Self::Err { code, message, allowed_folders }
    }
}

/// [`BusError`] variants that must survive the wire with their meaning intact
/// (callers match on them). Everything else collapses to `Other`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireErrorCode {
    NoDaemon,
    NoDispatcher,
    Closed,
    Disconnected,
    Timeout,
    Staging,
    ListDirs,
    GitInfo,
    ReadFile { kind: ReadFileErrorKind },
    Other,
}

/// Encode a [`BusError`] for the route response. Payload-carrying variants
/// (`Staging`/`ListDirs`) ship their inner message so [`decode_error`] can
/// rebuild the variant without double-wrapping the display prefix.
pub fn encode_error(err: &BusError) -> (WireErrorCode, String) {
    match err {
        BusError::NoDaemon(_) => (WireErrorCode::NoDaemon, err.to_string()),
        BusError::NoDispatcher(_) => (WireErrorCode::NoDispatcher, err.to_string()),
        BusError::Closed => (WireErrorCode::Closed, err.to_string()),
        BusError::Disconnected => (WireErrorCode::Disconnected, err.to_string()),
        BusError::Timeout => (WireErrorCode::Timeout, err.to_string()),
        BusError::Staging(msg) => (WireErrorCode::Staging, msg.clone()),
        BusError::ListDirs(msg) => (WireErrorCode::ListDirs, msg.clone()),
        BusError::GitInfo(msg) => (WireErrorCode::GitInfo, msg.clone()),
        BusError::ReadFile(r) => (WireErrorCode::ReadFile { kind: r.kind }, r.message.clone()),
        BusError::NotFound
        | BusError::NoAdapter
        | BusError::NoMachine
        | BusError::Db(_)
        | BusError::Reconcile(_)
        | BusError::Transport(_) => (WireErrorCode::Other, err.to_string()),
    }
}

/// Reconstruct a [`BusError`] from a peer's error response. `target` is the
/// machine/dispatcher uuid the caller addressed (the wire doesn't re-carry it).
pub fn decode_error(
    code: WireErrorCode,
    message: String,
    allowed_folders: Vec<String>,
    target: Uuid,
) -> BusError {
    match code {
        WireErrorCode::NoDaemon => BusError::NoDaemon(target),
        WireErrorCode::NoDispatcher => BusError::NoDispatcher(target),
        WireErrorCode::Closed => BusError::Closed,
        WireErrorCode::Disconnected => BusError::Disconnected,
        WireErrorCode::Timeout => BusError::Timeout,
        WireErrorCode::Staging => BusError::Staging(message),
        WireErrorCode::ListDirs => BusError::ListDirs(message),
        WireErrorCode::GitInfo => BusError::GitInfo(message),
        WireErrorCode::ReadFile { kind } => {
            BusError::ReadFile(cctui_proto::ws::ReadFileRefusal { kind, message, allowed_folders })
        }
        // An unclassified peer-side failure still means the frame was not
        // delivered; surface the peer's message verbatim.
        WireErrorCode::Other => BusError::Transport(message),
    }
}

/// Wire form of [`BusEvent`] for `POST /internal/bus/publish` (a batch of
/// these). `BusEvent` itself stays wire-agnostic; this owns the serde shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireBusEvent {
    Session { session_id: String, event: AgentEvent },
    Server { event: ServerEvent },
    Voice { event: VoiceEvent },
}

impl From<&BusEvent> for WireBusEvent {
    fn from(ev: &BusEvent) -> Self {
        match ev {
            BusEvent::Session { session_id, event } => {
                Self::Session { session_id: session_id.clone(), event: event.clone() }
            }
            BusEvent::Server(event) => Self::Server { event: event.clone() },
            BusEvent::Voice(event) => Self::Voice { event: event.clone() },
        }
    }
}

impl From<WireBusEvent> for BusEvent {
    fn from(ev: WireBusEvent) -> Self {
        match ev {
            WireBusEvent::Session { session_id, event } => Self::Session { session_id, event },
            WireBusEvent::Server { event } => Self::Server(event),
            WireBusEvent::Voice { event } => Self::Voice(event),
        }
    }
}

/// The session a down-frame belongs to, when it is session-scoped.
pub use cctui_server_core::bus::frame_session;
/// Format a peer IP + port as an HTTP authority (IPv6 literals need brackets).
fn peer_base(ip: &str, port: u16) -> String {
    if ip.contains(':') { format!("http://[{ip}]:{port}") } else { format!("http://{ip}:{port}") }
}

// ---- the transport ----

/// Cross-replica transport over pod-to-pod HTTP. Constructed in `main` when
/// `CCTUI_POD_IP` is set; local dev / single replicas keep [`super::NoopTransport`].
pub struct PeerHttpTransport {
    pool: PgPool,
    client: reqwest::Client,
    /// This pod's name — excluded from peer lookups so we never forward to
    /// ourselves.
    pod: String,
    /// The port every replica's HTTP server listens on (identical across the
    /// deployment, same assumption the forwarder made).
    port: u16,
    /// The cluster-internal shared secret (Bearer on every internal call).
    secret: String,
    /// Queue into the background relay worker, as pre-serialized events.
    relay_tx: mpsc::Sender<Arc<str>>,
    /// Events dropped because the relay ingress was full.
    relay_dropped: AtomicU64,
}

impl PeerHttpTransport {
    pub fn new(
        pool: PgPool,
        client: reqwest::Client,
        pod: String,
        port: u16,
        secret: String,
    ) -> Self {
        let (relay_tx, relay_rx) = mpsc::channel(RELAY_QUEUE);
        tokio::spawn(relay_worker(
            pool.clone(),
            client.clone(),
            pod.clone(),
            port,
            secret.clone(),
            relay_rx,
        ));
        Self { pool, client, pod, port, secret, relay_tx, relay_dropped: AtomicU64::new(0) }
    }

    /// The miss error for `kind` — what the caller would have seen locally.
    const fn miss(kind: Kind, target: Uuid) -> BusError {
        match kind {
            Kind::Daemon | Kind::Session => BusError::NoDaemon(target),
            Kind::Dispatcher => BusError::NoDispatcher(target),
        }
    }

    /// The owner row for `target`: the pod that announced the session when the
    /// frame names one, else the pod owning the machine or dispatcher. A machine
    /// id can be shared by several daemons, so its row alone would pick an
    /// arbitrary one.
    async fn resolve(&self, target: Target) -> Option<Owner> {
        if let Some(session) = target.session
            && let Some(ip) =
                presence::peer_owner_ip(&self.pool, &self.pod, Kind::Session, session).await
        {
            return Some(Owner { kind: Kind::Session, id: session, ip });
        }
        presence::peer_owner_ip(&self.pool, &self.pod, target.kind, target.id)
            .await
            .map(|ip| Owner { kind: target.kind, id: target.id, ip })
    }

    /// Look up the live peer owning `target`'s WS and POST the route request to
    /// it. No live owner, a rejected request, or a moved connection all map to
    /// the honest miss error; peer-side delivery failures are decoded back into
    /// their [`BusError`] variants. An owner that cannot be reached at all is a
    /// replaced pod: its row is forgotten (only while it still names that IP)
    /// and the request waits up to [`REOWN_WAIT`] for the daemon's reconnect to
    /// record a new owner, then is retried once there.
    async fn route(
        &self,
        target: Target,
        request: &RouteRequest,
        timeout: std::time::Duration,
    ) -> Result<RouteResponse, BusError> {
        let Some(owner) = self.resolve(target).await else {
            return Err(Self::miss(target.kind, target.id));
        };
        if let Some(response) = self.post(&owner, target, request, timeout).await? {
            return Ok(response);
        }
        self.forget(&owner, target).await;
        let Some(fresh) = self.await_replacement(target, &owner.ip).await else {
            return Err(Self::miss(target.kind, target.id));
        };
        if let Some(response) = self.post(&fresh, target, request, timeout).await? {
            return Ok(response);
        }
        self.forget(&fresh, target).await;
        Err(Self::miss(target.kind, target.id))
    }

    async fn forget(&self, owner: &Owner, target: Target) {
        let dropped = presence::forget_owner(&self.pool, owner.kind, owner.id, &owner.ip).await;
        tracing::warn!(
            owner = %owner.ip,
            target = %target.id,
            kind = target.kind.as_str(),
            dropped,
            "peer bus owner unreachable"
        );
    }

    /// Re-read the owner until one naming another IP than `stale` appears or
    /// [`REOWN_WAIT`] runs out.
    async fn await_replacement(&self, target: Target, stale: &str) -> Option<Owner> {
        await_replacement(stale, REOWN_WAIT, REOWN_POLL, || self.resolve(target)).await
    }

    /// One POST to one owner. `Err` is a final bus error; `Ok(None)` is a
    /// connection-level failure (nothing listens at the owner's IP) the caller
    /// may retry elsewhere.
    async fn post(
        &self,
        owner: &Owner,
        target: Target,
        request: &RouteRequest,
        timeout: std::time::Duration,
    ) -> Result<Option<RouteResponse>, BusError> {
        let url = format!("{}/internal/bus/route", peer_base(&owner.ip, self.port));
        let response = match self
            .client
            .post(&url)
            .bearer_auth(&self.secret)
            .timeout(timeout)
            .json(request)
            .send()
            .await
        {
            Ok(r) => r,
            Err(err) if err.is_connect() => return Ok(None),
            Err(err) => {
                tracing::warn!(
                    %err,
                    owner = %owner.ip,
                    target = %target.id,
                    kind = target.kind.as_str(),
                    "peer bus route failed"
                );
                return Err(Self::miss(target.kind, target.id));
            }
        };
        if !response.status().is_success() {
            tracing::warn!(
                status = %response.status(),
                owner = %owner.ip,
                target = %target.id,
                "peer bus route rejected"
            );
            return Err(Self::miss(target.kind, target.id));
        }
        match response.json::<RouteResponse>().await {
            Ok(RouteResponse::Err { code, message, allowed_folders }) => {
                Err(decode_error(code, message, allowed_folders, target.id))
            }
            Ok(ok) => Ok(Some(ok)),
            Err(err) => {
                tracing::warn!(%err, owner = %owner.ip, "peer bus route response unreadable");
                Err(BusError::Transport(format!("peer response unreadable: {err}")))
            }
        }
    }
}

/// The WS a route request is addressed to. `session` is the announced session
/// a daemon frame names, whose own owner row wins over the machine's.
#[derive(Debug, Clone, Copy)]
struct Target {
    kind: Kind,
    id: Uuid,
    session: Option<Uuid>,
}

impl Target {
    fn machine(id: Uuid, session: Option<&str>) -> Self {
        let session = session.and_then(|s| Uuid::parse_str(s).ok());
        Self { kind: Kind::Daemon, id, session }
    }

    const fn dispatcher(id: Uuid) -> Self {
        Self { kind: Kind::Dispatcher, id, session: None }
    }
}

/// A resolved owner row: which row named `ip`, so forgetting a dead owner
/// touches that row and no other.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Owner {
    kind: Kind,
    id: Uuid,
    ip: String,
}

/// Poll `lookup` every `poll` until it names an owner other than `stale`, or
/// `wait` has elapsed. `None` from `lookup` (no owner yet) keeps polling: the
/// daemon's reconnect registers its new owner a moment after the old pod's row
/// is gone.
async fn await_replacement<F, Fut>(
    stale: &str,
    wait: std::time::Duration,
    poll: std::time::Duration,
    mut lookup: F,
) -> Option<Owner>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<Owner>>,
{
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        tokio::time::sleep(poll).await;
        if let Some(owner) = lookup().await
            && owner.ip != stale
        {
            return Some(owner);
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
    }
}

#[async_trait::async_trait]
impl Transport for PeerHttpTransport {
    async fn forward_daemon(&self, machine: Uuid, frame: DaemonFrameDown) -> Result<(), BusError> {
        let target = Target::machine(machine, frame_session(&frame));
        let request = RouteRequest::DaemonCommand { machine, frame };
        match self.route(target, &request, COMMAND_TIMEOUT).await? {
            RouteResponse::Ok => Ok(()),
            other => Err(BusError::Transport(format!("unexpected peer reply: {other:?}"))),
        }
    }

    async fn forward_dispatcher(
        &self,
        dispatcher: Uuid,
        frame: DispatcherFrameDown,
    ) -> Result<(), BusError> {
        let request = RouteRequest::DispatcherCommand { dispatcher, frame };
        match self.route(Target::dispatcher(dispatcher), &request, COMMAND_TIMEOUT).await? {
            RouteResponse::Ok => Ok(()),
            other => Err(BusError::Transport(format!("unexpected peer reply: {other:?}"))),
        }
    }

    async fn request_daemon(
        &self,
        machine: Uuid,
        request: DaemonRequest,
    ) -> Result<DaemonResponse, BusError> {
        let session = match &request {
            DaemonRequest::StageFiles { local_id, .. }
            | DaemonRequest::Diagnose { local_id, .. } => Some(local_id.clone()),
            _ => None,
        };
        let target = Target::machine(machine, session.as_deref());
        let (request, timeout) = match request {
            DaemonRequest::StageFiles { adapter_id, local_id, uploads } => (
                RouteRequest::DaemonStageFiles { machine, adapter_id, local_id, uploads },
                STAGE_FORWARD_TIMEOUT,
            ),
            DaemonRequest::ListDirs { path } => {
                (RouteRequest::DaemonListDirs { machine, path }, LIST_DIRS_FORWARD_TIMEOUT)
            }
            DaemonRequest::GitInfo { path, include_dirty } => (
                RouteRequest::DaemonGitInfo { machine, path, include_dirty },
                LIST_DIRS_FORWARD_TIMEOUT,
            ),
            DaemonRequest::ReadFile { path, max_bytes, cwd } => (
                RouteRequest::DaemonReadFile { machine, path, max_bytes, cwd },
                READ_FILE_FORWARD_TIMEOUT,
            ),
            DaemonRequest::Diagnose { adapter_id, local_id } => (
                RouteRequest::DaemonDiagnose { machine, adapter_id, local_id },
                DIAGNOSE_FORWARD_TIMEOUT,
            ),
        };
        match self.route(target, &request, timeout).await? {
            RouteResponse::StagedFiles { paths } => Ok(DaemonResponse::StagedFiles(paths)),
            RouteResponse::Dirs { dirs } => Ok(DaemonResponse::Dirs(dirs)),
            RouteResponse::GitInfo { info } => Ok(DaemonResponse::GitInfo(info)),
            RouteResponse::File { file } => Ok(DaemonResponse::File(file)),
            RouteResponse::Diagnose { report } => Ok(DaemonResponse::Diagnose(report)),
            other => Err(BusError::Transport(format!("unexpected peer reply: {other:?}"))),
        }
    }

    async fn request_dispatcher(
        &self,
        dispatcher: Uuid,
        request_id: Uuid,
        frame: DispatcherFrameDown,
    ) -> Result<DispatcherFrameUp, BusError> {
        let request = RouteRequest::DispatcherRequest { dispatcher, request_id, frame };
        let target = Target::dispatcher(dispatcher);
        match self.route(target, &request, DISPATCHER_FORWARD_TIMEOUT).await? {
            RouteResponse::DispatcherReply { frame } => Ok(frame),
            other => Err(BusError::Transport(format!("unexpected peer reply: {other:?}"))),
        }
    }

    async fn session_elsewhere(&self, session_id: &str) -> bool {
        let Ok(session) = Uuid::parse_str(session_id) else { return false };
        presence::peer_owner_ip(&self.pool, &self.pod, Kind::Session, session).await.is_some()
    }

    fn relay(&self, event: &BusEvent) {
        let json = match serde_json::to_string(&WireBusEvent::from(event)) {
            Ok(json) => Arc::<str>::from(json),
            Err(err) => {
                tracing::warn!(%err, "bus event not serializable for relay");
                return;
            }
        };
        if let Err(mpsc::error::TrySendError::Full(_)) = self.relay_tx.try_send(json) {
            note_drop(&self.relay_dropped, "ingress");
        }
    }
}

fn note_drop(counter: &AtomicU64, peer: &str) {
    let dropped = counter.fetch_add(1, Ordering::Relaxed) + 1;
    if dropped.is_power_of_two() {
        tracing::warn!(%peer, dropped, "peer bus relay queue full, dropping events");
    }
}

/// Background fan-out: route each relayed event into every live peer's own
/// queue. Best-effort — the DB remains the recovery path (clients refetch on
/// resubscribe).
async fn relay_worker(
    pool: PgPool,
    client: reqwest::Client,
    pod: String,
    port: u16,
    secret: String,
    mut rx: mpsc::Receiver<Arc<str>>,
) {
    let mut fanout = RelayFanout::default();
    let mut peers_fetched_at: Option<std::time::Instant> = None;
    while let Some(event) = rx.recv().await {
        if peers_fetched_at.is_none_or(|t| t.elapsed() > PEER_CACHE_TTL) {
            let peers = presence::live_peer_pods(&pool, &pod).await;
            fanout.sync_peers(&peers, |ip| {
                spawn_peer_relay(client.clone(), peer_base(ip, port), secret.clone())
            });
            peers_fetched_at = Some(std::time::Instant::now());
        }
        fanout.push(&event);
    }
}

struct PeerQueue {
    tx: mpsc::Sender<Arc<str>>,
    dropped: AtomicU64,
}

/// One bounded queue per live peer, so a slow peer sheds its own backlog
/// without stalling the others.
#[derive(Default)]
struct RelayFanout {
    peers: HashMap<String, PeerQueue>,
}

impl RelayFanout {
    /// Keep a queue for exactly `live` peers. Dropping a departed peer's sender
    /// ends its task.
    fn sync_peers(
        &mut self,
        live: &[String],
        mut spawn: impl FnMut(&str) -> mpsc::Sender<Arc<str>>,
    ) {
        self.peers.retain(|ip, _| live.contains(ip));
        for ip in live {
            if !self.peers.contains_key(ip) {
                let tx = spawn(ip);
                self.peers.insert(ip.clone(), PeerQueue { tx, dropped: AtomicU64::new(0) });
            }
        }
    }

    fn push(&self, event: &Arc<str>) {
        for (ip, peer) in &self.peers {
            if let Err(mpsc::error::TrySendError::Full(_)) = peer.tx.try_send(Arc::clone(event)) {
                note_drop(&peer.dropped, ip);
            }
        }
    }
}

fn spawn_peer_relay(
    client: reqwest::Client,
    base: String,
    secret: String,
) -> mpsc::Sender<Arc<str>> {
    let (tx, rx) = mpsc::channel(RELAY_QUEUE);
    tokio::spawn(peer_relay(client, base, secret, rx));
    tx
}

/// Drain one peer's queue in batches, `POSTing` each as a JSON array.
async fn peer_relay(
    client: reqwest::Client,
    base: String,
    secret: String,
    mut rx: mpsc::Receiver<Arc<str>>,
) {
    let url = format!("{base}/internal/bus/publish");
    let mut batch = Vec::with_capacity(RELAY_BATCH);
    while rx.recv_many(&mut batch, RELAY_BATCH).await > 0 {
        let mut body = String::from("[");
        for (i, event) in batch.iter().enumerate() {
            if i > 0 {
                body.push(',');
            }
            body.push_str(event);
        }
        body.push(']');
        batch.clear();
        let sent = client
            .post(&url)
            .bearer_auth(&secret)
            .timeout(RELAY_TIMEOUT)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await;
        match sent {
            Ok(r) if r.status().is_success() => {}
            Ok(r) => {
                tracing::warn!(peer = %base, status = %r.status(), "peer bus publish rejected");
            }
            Err(err) => tracing::warn!(peer = %base, %err, "peer bus publish failed"),
        }
    }
}

/// Sends auth-cache purges straight to every live peer's `/internal/auth/purge`.
///
/// Bypasses the batched event relay so a revocation is never queued behind an
/// event backlog.
pub struct PeerAuthPurge {
    pool: PgPool,
    client: reqwest::Client,
    pod: String,
    port: u16,
    secret: String,
}

impl PeerAuthPurge {
    pub const fn new(
        pool: PgPool,
        client: reqwest::Client,
        pod: String,
        port: u16,
        secret: String,
    ) -> Self {
        Self { pool, client, pod, port, secret }
    }
}

impl crate::auth::PurgeRelay for PeerAuthPurge {
    fn relay(&self, purge: crate::auth::CachePurge) {
        let pool = self.pool.clone();
        let client = self.client.clone();
        let pod = self.pod.clone();
        let port = self.port;
        let secret = self.secret.clone();
        tokio::spawn(async move {
            let peers = presence::live_peer_pods(&pool, &pod).await;
            let bases: Vec<String> = peers.iter().map(|ip| peer_base(ip, port)).collect();
            send_auth_purge(&client, &bases, &secret, &purge).await;
        });
    }
}

/// POST `purge` to every peer in `bases` concurrently. Failures are logged
/// only: the peer's entry then lives out its TTL.
async fn send_auth_purge(
    client: &reqwest::Client,
    bases: &[String],
    secret: &str,
    purge: &crate::auth::CachePurge,
) {
    post_to_peers(client, bases, secret, "auth/purge", purge).await;
}

/// POST `body` to `/internal/{path}` on every peer in `bases` concurrently,
/// logging failures only.
async fn post_to_peers<T: Serialize + Sync>(
    client: &reqwest::Client,
    bases: &[String],
    secret: &str,
    path: &str,
    body: &T,
) {
    futures_util::future::join_all(bases.iter().map(|base| async move {
        let sent = client
            .post(format!("{base}/internal/{path}"))
            .bearer_auth(secret)
            .timeout(AUTH_PURGE_TIMEOUT)
            .json(body)
            .send()
            .await;
        match sent {
            Ok(r) if r.status().is_success() => {}
            Ok(r) => tracing::warn!(peer = %base, status = %r.status(), path, "peer post rejected"),
            Err(err) => tracing::warn!(peer = %base, %err, path, "peer post failed"),
        }
    }))
    .await;
}

/// Sends guard-step forgets straight to every live peer's
/// `/internal/guard/forget`. A peer that predates the route answers 404 and
/// keeps its entry until it restarts.
pub struct PeerGuardForget {
    pool: PgPool,
    client: reqwest::Client,
    pod: String,
    port: u16,
    secret: String,
}

impl PeerGuardForget {
    pub const fn new(
        pool: PgPool,
        client: reqwest::Client,
        pod: String,
        port: u16,
        secret: String,
    ) -> Self {
        Self { pool, client, pod, port, secret }
    }
}

impl crate::guard_step::ForgetRelay for PeerGuardForget {
    fn relay(&self, forget: crate::guard_step::GuardForget) {
        let pool = self.pool.clone();
        let client = self.client.clone();
        let pod = self.pod.clone();
        let port = self.port;
        let secret = self.secret.clone();
        tokio::spawn(async move {
            let peers = presence::live_peer_pods(&pool, &pod).await;
            let bases: Vec<String> = peers.iter().map(|ip| peer_base(ip, port)).collect();
            post_to_peers(&client, &bases, &secret, "guard/forget", &forget).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn stub_peer(delay: std::time::Duration, received: Arc<AtomicU64>) -> String {
        let app = axum::Router::new().route(
            "/internal/bus/publish",
            axum::routing::post(move |axum::Json(batch): axum::Json<Vec<WireBusEvent>>| {
                let received = received.clone();
                async move {
                    tokio::time::sleep(delay).await;
                    received.fetch_add(u64::try_from(batch.len()).unwrap(), Ordering::SeqCst);
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        addr.to_string()
    }

    #[tokio::test]
    async fn a_hung_peer_sheds_its_own_backlog_without_starving_a_healthy_one() {
        let slow_rx = Arc::new(AtomicU64::new(0));
        let fast_rx = Arc::new(AtomicU64::new(0));
        let slow = stub_peer(std::time::Duration::from_secs(10), slow_rx.clone()).await;
        let fast = stub_peer(std::time::Duration::ZERO, fast_rx.clone()).await;
        let client = crate::build_http_client();
        let mut fanout = RelayFanout::default();
        fanout.sync_peers(&[slow.clone(), fast.clone()], |addr| {
            spawn_peer_relay(client.clone(), format!("http://{addr}"), "s".into())
        });

        let event = BusEvent::Server(ServerEvent::AskResolved { session_id: "sess-1".into() });
        let json: Arc<str> = serde_json::to_string(&WireBusEvent::from(&event)).unwrap().into();
        let total = 10_000u64;
        for i in 0..total {
            fanout.push(&json);
            if i % 256 == 0 {
                tokio::task::yield_now().await;
            }
        }

        let slow_dropped = fanout.peers[&slow].dropped.load(Ordering::SeqCst);
        let slow_room = u64::try_from(RELAY_QUEUE + 2 * RELAY_BATCH).unwrap();
        assert!(slow_dropped >= total - slow_room, "{slow_dropped}");

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while fast_rx.load(Ordering::SeqCst) == 0 && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(fast_rx.load(Ordering::SeqCst) > 0, "healthy peer received nothing");
        assert_eq!(slow_rx.load(Ordering::SeqCst), 0);

        fanout.sync_peers(std::slice::from_ref(&fast), |_| unreachable!());
        assert!(!fanout.peers.contains_key(&slow));
    }

    /// Cross-replica forwarding keys on the session, so a machine-scoped
    /// frame and a session-scoped one must be told apart from the frame alone.
    #[test]
    fn frame_session_reads_the_target_of_session_scoped_frames() {
        let reply = DaemonFrameDown::Command {
            adapter_id: "claude-code".into(),
            command: Box::new(cctui_proto::adapter::AdapterCommand::Reply {
                local_id: "sess-1".into(),
                recover_auth: false,
                text: "hi".into(),
                ask_picks: None,
                env: std::collections::BTreeMap::new(),
                command_id: None,
                turn_id: None,
            }),
        };
        assert_eq!(frame_session(&reply), Some("sess-1"));

        let reconcile = DaemonFrameDown::Reconcile {
            adapters: Vec::new(),
            secret_scrub: cctui_proto::ws::SecretScrubConfig::default(),
        };
        assert_eq!(frame_session(&reconcile), None);

        let spawn = DaemonFrameDown::Command {
            adapter_id: "claude-code".into(),
            command: Box::new(cctui_proto::adapter::AdapterCommand::ResumeMarks {
                marks: Vec::new(),
            }),
        };
        assert_eq!(frame_session(&spawn), None, "adapter-wide commands stay machine-scoped");

        let ack = DaemonFrameDown::TranscriptAck {
            adapter_id: "claude-code".into(),
            session_marks: vec![("sess-1".into(), 4096)],
        };
        assert_eq!(frame_session(&ack), Some("sess-1"), "an ack belongs to its session's daemon");
    }

    #[test]
    fn error_codes_round_trip_meaningfully() {
        let machine = Uuid::new_v4();
        let cases: Vec<BusError> = vec![
            BusError::NoDaemon(machine),
            BusError::Closed,
            BusError::Disconnected,
            BusError::Timeout,
            BusError::Staging("disk full".into()),
            BusError::ListDirs("no such directory".into()),
        ];
        for err in cases {
            let (code, message) = encode_error(&err);
            let back = decode_error(code, message, Vec::new(), machine);
            assert_eq!(
                std::mem::discriminant(&err),
                std::mem::discriminant(&back),
                "{err:?} -> {back:?}"
            );
            assert_eq!(err.to_string(), back.to_string());
        }
    }

    #[test]
    fn dispatcher_miss_decodes_with_target() {
        let dispatcher = Uuid::new_v4();
        let (code, message) = encode_error(&BusError::NoDispatcher(dispatcher));
        let back = decode_error(code, message, Vec::new(), dispatcher);
        assert!(matches!(back, BusError::NoDispatcher(d) if d == dispatcher));
    }

    #[test]
    fn unclassified_errors_collapse_to_transport() {
        let (code, message) = encode_error(&BusError::Reconcile("boom".into()));
        assert_eq!(code, WireErrorCode::Other);
        let back = decode_error(code, message, Vec::new(), Uuid::new_v4());
        assert!(matches!(back, BusError::Transport(m) if m.contains("boom")));
    }

    #[test]
    fn wire_bus_event_round_trips() {
        let ev = BusEvent::Server(ServerEvent::AskResolved { session_id: "sess-1".into() });
        let wire = WireBusEvent::from(&ev);
        let json = serde_json::to_string(&wire).unwrap();
        assert!(json.contains(r#""kind":"server""#));
        let back: WireBusEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            BusEvent::from(back),
            BusEvent::Server(ServerEvent::AskResolved { session_id }) if session_id == "sess-1"
        ));

        let ev = BusEvent::Session {
            session_id: "sess-2".into(),
            event: AgentEvent::TurnEnd { ts: 7, seq: None },
        };
        let wire = WireBusEvent::from(&ev);
        let json = serde_json::to_string(&wire).unwrap();
        assert!(json.contains(r#""kind":"session""#));
        let back: WireBusEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            BusEvent::from(back),
            BusEvent::Session { session_id, event: AgentEvent::TurnEnd { ts: 7, .. } }
                if session_id == "sess-2"
        ));

        let delta = VoiceEvent::Delta {
            session_id: "sess-3".into(),
            item_id: "gw:r:0".into(),
            delta: "Hi".into(),
        };
        let json =
            serde_json::to_string(&WireBusEvent::from(&BusEvent::Voice(delta.clone()))).unwrap();
        assert!(json.contains(r#""kind":"voice""#), "{json}");
        let back: WireBusEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(BusEvent::from(back), BusEvent::Voice(ev) if ev == delta));
    }

    #[test]
    fn peer_base_brackets_ipv6() {
        assert_eq!(peer_base("10.0.0.1", 8700), "http://10.0.0.1:8700");
        assert_eq!(peer_base("fd00::1", 8700), "http://[fd00::1]:8700");
    }

    #[test]
    fn route_request_serde_shape() {
        let req = RouteRequest::DaemonListDirs { machine: Uuid::nil(), path: "/home".into() };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains(r#""op":"daemon_list_dirs""#));
        let _back: RouteRequest = serde_json::from_str(&json).unwrap();

        let resp = RouteResponse::Err {
            code: WireErrorCode::NoDaemon,
            message: "no daemon connected".into(),
            allowed_folders: Vec::new(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains(r#""outcome":"err""#));
        assert!(json.contains(r#""code":"no_daemon""#));
        let _back: RouteResponse = serde_json::from_str(&json).unwrap();
        assert!(!json.contains("allowed_folders"), "an empty list stays off the wire: {json}");
    }

    #[test]
    fn forwarded_read_denial_keeps_allowed_folders() {
        let machine = Uuid::new_v4();
        let err = BusError::ReadFile(cctui_proto::ws::ReadFileRefusal {
            kind: ReadFileErrorKind::Denied,
            message: "outside allowed folders".into(),
            allowed_folders: vec!["/tmp".into(), "/srv/app".into()],
        });
        let json = serde_json::to_string(&RouteResponse::from_error(&err)).unwrap();
        let RouteResponse::Err { code, message, allowed_folders } =
            serde_json::from_str(&json).unwrap()
        else {
            panic!("not an err: {json}")
        };
        let back = decode_error(code, message, allowed_folders, machine);
        assert!(matches!(back, BusError::ReadFile(r)
            if r.kind == ReadFileErrorKind::Denied && r.allowed_folders == ["/tmp", "/srv/app"]));
    }

    #[test]
    fn read_denial_from_an_older_peer_still_decodes() {
        let json = r#"{"outcome":"err","code":{"read_file":{"kind":"denied"}},"message":"no"}"#;
        let RouteResponse::Err { code, message, allowed_folders } =
            serde_json::from_str(json).unwrap()
        else {
            panic!("not an err")
        };
        assert!(allowed_folders.is_empty());
        let back = decode_error(code, message, allowed_folders, Uuid::new_v4());
        assert!(matches!(back, BusError::ReadFile(r) if r.message == "no"));
    }

    fn cached_ctx() -> crate::auth::AuthContext {
        crate::auth::AuthContext {
            user_id: Uuid::new_v4(),
            key_id: Uuid::new_v4(),
            machine_id: None,
            scopes: std::iter::once(crate::auth::Scope::Read).collect(),
        }
    }

    /// A replica serving the real `/internal/auth/purge` handler, with `hash`
    /// cached as live. Returns its state and base URL.
    async fn replica_with_cached(hash: &str) -> (crate::state::AppState, String) {
        let pool = PgPool::connect_lazy("postgres://invalid").unwrap();
        let mut state = crate::state::AppState::for_test(pool);
        state.internal_secret = Some(Arc::from("shared"));
        state.auth_config.seed_cache(hash, cached_ctx());
        let app = axum::Router::new()
            .route("/internal/auth/purge", axum::routing::post(crate::routes::internal::auth_purge))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (state, format!("http://{addr}"))
    }

    struct DirectPeers {
        client: reqwest::Client,
        bases: Vec<String>,
    }

    impl crate::auth::PurgeRelay for DirectPeers {
        fn relay(&self, purge: crate::auth::CachePurge) {
            let client = self.client.clone();
            let bases = self.bases.clone();
            tokio::spawn(async move { send_auth_purge(&client, &bases, "shared", &purge).await });
        }
    }

    #[tokio::test]
    async fn a_revoke_on_one_replica_drops_the_key_from_the_other() {
        let (replica_b, base) = replica_with_cached("revoked").await;
        replica_b.auth_config.seed_cache("other", cached_ctx());

        let replica_a =
            crate::state::AppState::for_test(PgPool::connect_lazy("postgres://invalid").unwrap());
        replica_a
            .auth_config
            .set_peers(Arc::new(DirectPeers { client: reqwest::Client::new(), bases: vec![base] }));
        replica_a.auth_config.purge("revoked");

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while replica_b.auth_config.is_cached("revoked") {
            assert!(tokio::time::Instant::now() < deadline, "peer purge never landed");
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(replica_b.auth_config.is_cached("other"), "only the revoked hash is dropped");

        replica_a.auth_config.purge_all();
        while replica_b.auth_config.is_cached("other") {
            assert!(tokio::time::Instant::now() < deadline, "peer purge-all never landed");
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn a_purge_with_the_wrong_secret_is_refused() {
        let (replica, base) = replica_with_cached("h").await;
        let purge = crate::auth::CachePurge::Key { key_hash: "h".into() };
        send_auth_purge(&reqwest::Client::new(), &[base], "wrong", &purge).await;
        assert!(replica.auth_config.is_cached("h"));
    }

    async fn guard_replica() -> String {
        let mut state =
            crate::state::AppState::for_test(PgPool::connect_lazy("postgres://invalid").unwrap());
        state.internal_secret = Some(Arc::from("shared"));
        let app = axum::Router::new()
            .route(
                "/internal/guard/forget",
                axum::routing::post(crate::routes::internal::guard_forget),
            )
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn a_guard_forget_reaches_the_peer_route_with_the_shared_secret() {
        let base = guard_replica().await;
        let step = cctui_proto::guard_step::GuardStepState { step: 2, ..Default::default() };
        let forget = crate::guard_step::GuardForget { session_id: "peer-guard-forget".into() };
        crate::guard_step::record(forget.session_id.clone(), step);

        post_to_peers(
            &reqwest::Client::new(),
            std::slice::from_ref(&base),
            "wrong",
            "guard/forget",
            &forget,
        )
        .await;
        assert!(crate::guard_step::get("peer-guard-forget").is_some(), "wrong secret refused");

        post_to_peers(&reqwest::Client::new(), &[base], "shared", "guard/forget", &forget).await;
        assert_eq!(crate::guard_step::get("peer-guard-forget"), None);
    }

    fn owner(ip: &str) -> Owner {
        Owner { kind: Kind::Daemon, id: Uuid::nil(), ip: ip.into() }
    }

    /// The wait returns as soon as a row names another IP; the stale IP being
    /// re-read (a late heartbeat) or no row at all (reconnect in flight) keeps
    /// polling until the deadline.
    #[tokio::test(start_paused = true)]
    async fn await_replacement_returns_the_first_owner_that_is_not_the_stale_one() {
        let reads = Arc::new(AtomicU64::new(0));
        let seen = reads.clone();
        let found = await_replacement("10.0.0.1", REOWN_WAIT, REOWN_POLL, move || {
            let n = seen.fetch_add(1, Ordering::SeqCst);
            async move {
                match n {
                    0 => None,
                    1 => Some(owner("10.0.0.1")),
                    _ => Some(owner("10.0.0.2")),
                }
            }
        })
        .await;
        assert_eq!(found, Some(owner("10.0.0.2")));
        assert_eq!(reads.load(Ordering::SeqCst), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn await_replacement_gives_up_at_the_deadline_when_only_the_stale_owner_is_seen() {
        let started = tokio::time::Instant::now();
        let found = await_replacement("10.0.0.1", REOWN_WAIT, REOWN_POLL, || async {
            Some(owner("10.0.0.1"))
        })
        .await;
        assert_eq!(found, None);
        assert!(started.elapsed() >= REOWN_WAIT);
        assert!(started.elapsed() < REOWN_WAIT + REOWN_POLL * 2);
    }

    #[tokio::test(start_paused = true)]
    async fn await_replacement_gives_up_at_the_deadline_without_any_owner() {
        let found = await_replacement("10.0.0.1", REOWN_WAIT, REOWN_POLL, || async { None }).await;
        assert_eq!(found, None);
    }

    /// `/internal/bus/route` that answers `Ok` and counts calls, bound on the
    /// given loopback address so two "pods" can share the transport's port.
    async fn stub_route_peer(ip: &str, received: Arc<AtomicU64>) -> u16 {
        let app = axum::Router::new().route(
            "/internal/bus/route",
            axum::routing::post(move |axum::Json(_): axum::Json<RouteRequest>| {
                let received = received.clone();
                async move {
                    received.fetch_add(1, Ordering::SeqCst);
                    axum::Json(RouteResponse::Ok)
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind((ip, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        port
    }

    async fn presence_pool() -> Option<PgPool> {
        let url = crate::routes::gateway::test_db_url("peer-route")?;
        Some(
            sqlx::postgres::PgPoolOptions::new()
                .max_connections(2)
                .connect(&url)
                .await
                .expect("connect test db"),
        )
    }

    async fn set_owner(pool: &PgPool, machine: Uuid, pod: &str, ip: &str) {
        sqlx::query(
            "INSERT INTO ws_presence (kind, entity_id, pod, pod_ip, connected_at, heartbeat_at) \
             VALUES ('daemon', $1, $2, $3, now(), now()) \
             ON CONFLICT (kind, entity_id) DO UPDATE SET \
               pod = EXCLUDED.pod, pod_ip = EXCLUDED.pod_ip, heartbeat_at = now()",
        )
        .bind(machine)
        .bind(pod)
        .bind(ip)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn owner_ip(pool: &PgPool, machine: Uuid) -> Option<String> {
        sqlx::query_scalar(
            "SELECT pod_ip FROM ws_presence WHERE kind = 'daemon' AND entity_id = $1",
        )
        .bind(machine)
        .fetch_optional(pool)
        .await
        .unwrap()
    }

    fn reconcile() -> DaemonFrameDown {
        DaemonFrameDown::Reconcile {
            adapters: Vec::new(),
            secret_scrub: cctui_proto::ws::SecretScrubConfig::default(),
        }
    }

    /// The recorded owner is a replaced pod (nothing listens on its IP): its
    /// row goes, and the daemon's reconnect — landing on a live pod during the
    /// wait — gets the request instead of a `NoDaemon` refusal.
    #[tokio::test]
    async fn an_unreachable_owner_is_forgotten_and_the_reconnected_owner_gets_the_retry() {
        let Some(pool) = presence_pool().await else { return };
        let received = Arc::new(AtomicU64::new(0));
        let port = stub_route_peer("127.0.0.1", received.clone()).await;
        let transport = PeerHttpTransport::new(
            pool.clone(),
            crate::build_http_client(),
            "cct1642-self".into(),
            port,
            "s".into(),
        );
        let machine = Uuid::new_v4();
        set_owner(&pool, machine, "cct1642-dead", "127.0.0.2").await;
        tokio::spawn({
            let pool = pool.clone();
            async move {
                tokio::time::sleep(std::time::Duration::from_millis(600)).await;
                set_owner(&pool, machine, "cct1642-live", "127.0.0.1").await;
            }
        });

        transport.forward_daemon(machine, reconcile()).await.unwrap();
        assert_eq!(received.load(Ordering::SeqCst), 1);
        assert_eq!(owner_ip(&pool, machine).await.as_deref(), Some("127.0.0.1"));
        sqlx::query("DELETE FROM ws_presence WHERE entity_id = $1")
            .bind(machine)
            .execute(&pool)
            .await
            .unwrap();
    }

    /// No reconnect within the wait: the stale row is still dropped and the
    /// caller gets the same `NoDaemon` miss a local registry miss yields.
    #[tokio::test]
    async fn an_unreachable_owner_without_a_replacement_is_dropped_and_misses() {
        let Some(pool) = presence_pool().await else { return };
        let port = stub_route_peer("127.0.0.1", Arc::new(AtomicU64::new(0))).await;
        let transport = PeerHttpTransport::new(
            pool.clone(),
            crate::build_http_client(),
            "cct1642-self".into(),
            port,
            "s".into(),
        );
        let machine = Uuid::new_v4();
        set_owner(&pool, machine, "cct1642-dead", "127.0.0.2").await;

        let err = transport.forward_daemon(machine, reconcile()).await.unwrap_err();
        assert!(matches!(err, BusError::NoDaemon(m) if m == machine), "{err:?}");
        assert_eq!(owner_ip(&pool, machine).await, None);
    }

    #[tokio::test]
    async fn an_unreachable_peer_does_not_stall_the_others() {
        let (replica, base) = replica_with_cached("h").await;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let dead = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let purge = crate::auth::CachePurge::Key { key_hash: "h".into() };
        send_auth_purge(&reqwest::Client::new(), &[dead, base], "shared", &purge).await;
        assert!(!replica.auth_config.is_cached("h"));
    }
}
