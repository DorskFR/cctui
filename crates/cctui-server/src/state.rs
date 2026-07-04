use std::sync::Arc;

use cctui_proto::models::MachineLiveness;
use cctui_proto::ws::{DaemonFrameDown, DispatcherFrameDown, DispatcherFrameUp, ServerEvent};
use dashmap::DashMap;
use sqlx::PgPool;
use tokio::sync::{broadcast, mpsc};
use uuid::Uuid;

use crate::archive_store::ArchiveStore;
use crate::auth::AuthConfig;
use crate::config::Config;
use crate::dispatchers::Registry as DispatcherRegistry;
use crate::registry::SharedRegistry;
use crate::routes::permissions::SharedPermissionStore;
use crate::skill_store::SkillStore;

/// Per-machine outbound channel into the connected daemon's WS task.
/// Clients dispatch `DaemonFrameDown` commands by looking up the target
/// machine here. Absent entry = daemon offline; command should be written
/// to the `commands` table for replay on reconnect (post-v0).
pub type DaemonConnections = Arc<DashMap<Uuid, mpsc::Sender<DaemonFrameDown>>>;

/// Outcome of a mid-chat file-stage request (CCT-236): the staged absolute
/// paths on success, or an error string on failure.
pub type StageFilesOutcome = Result<Vec<String>, String>;

/// In-flight `POST /sessions/{id}/files` requests awaiting a daemon
/// `StageFilesResult`, keyed by the request id minted by the route. The daemon
/// WS read loop fires the oneshot when the matching reply arrives (CCT-236).
pub type PendingStageRequests = Arc<DashMap<Uuid, tokio::sync::oneshot::Sender<StageFilesOutcome>>>;

/// Outcome of a working-dir autocomplete listing (spawn dialog): the
/// directory names on success, or an error string on failure.
pub type ListDirsOutcome = Result<Vec<String>, String>;

/// In-flight `GET /machines/{id}/fs/dirs` requests awaiting a daemon
/// `ListDirsResult`, keyed by the request id minted by the route. The daemon
/// WS read loop fires the oneshot when the matching reply arrives.
pub type PendingListDirsRequests =
    Arc<DashMap<Uuid, tokio::sync::oneshot::Sender<ListDirsOutcome>>>;

/// Per-dispatcher outbound channel into the connected enrolled dispatcher's WS
/// task (CCT-285), keyed by dispatcher id. Peer of [`DaemonConnections`].
/// Absent entry = dispatcher offline; a dispatch targeting it fails fast.
pub type DispatcherConnections = Arc<DashMap<Uuid, mpsc::Sender<DispatcherFrameDown>>>;

/// In-flight Dispatch/Status/Cancel round-trips awaiting a
/// [`DispatcherFrameUp`] reply, keyed by the request id minted by the dispatch
/// path. The dispatcher WS read loop fires the oneshot when the matching reply
/// arrives (CCT-285); mirrors `pending_stage_requests`.
pub type PendingDispatcherRequests =
    Arc<DashMap<Uuid, tokio::sync::oneshot::Sender<DispatcherFrameUp>>>;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub config: Config,
    pub registry: SharedRegistry,
    pub permission_store: SharedPermissionStore,
    /// Broadcast channel for server-initiated TUI events (e.g. permission requests).
    pub tui_tx: broadcast::Sender<ServerEvent>,
    #[allow(dead_code)]
    pub auth_config: AuthConfig,
    pub archive: Arc<ArchiveStore>,
    pub skills: Arc<SkillStore>,
    pub daemon_connections: DaemonConnections,
    /// Connected enrolled dispatchers (CCT-285), keyed by dispatcher id.
    pub dispatcher_connections: DispatcherConnections,
    /// In-flight Dispatch/Status/Cancel round-trips awaiting a dispatcher reply.
    pub pending_dispatcher_requests: PendingDispatcherRequests,
    /// Last broadcast liveness tier per enrolled dispatcher (CCT-285), peer of
    /// [`Self::machine_liveness`].
    pub dispatcher_liveness: Arc<DashMap<Uuid, MachineLiveness>>,
    pub dispatchers: Arc<DispatcherRegistry>,
    /// In-flight mid-chat file-stage requests awaiting a daemon reply (CCT-236).
    pub pending_stage_requests: PendingStageRequests,
    /// In-flight working-dir autocomplete requests awaiting a daemon reply.
    pub pending_listdirs_requests: PendingListDirsRequests,
    /// Last broadcast liveness tier per machine (CCT-255). The daemon-WS
    /// heartbeat handler and the reaper both re-derive a machine's tier from
    /// `machines.last_seen_at` age; this map lets them broadcast a
    /// [`ServerEvent::MachineLiveness`] only on an actual transition (e.g.
    /// online → offline) rather than on every tick.
    pub machine_liveness: Arc<DashMap<Uuid, MachineLiveness>>,
    /// Per-OAuth-account refresh mutex (CCT-232). OAuth refresh tokens are
    /// single-use; two concurrent sessions on the same account must not both
    /// refresh (the second would invalidate the first). The gateway grabs the
    /// account's lock around the read-expiry → refresh → persist sequence.
    pub account_locks: Arc<DashMap<Uuid, Arc<tokio::sync::Mutex<()>>>>,
    /// Shared outbound HTTP client for the gateway passthrough (CCT-232).
    pub http_client: reqwest::Client,
    /// Optional Langfuse tracing sink for the `/gateway` proxy (CCT-443).
    /// `None` unless the `CCTUI_LANGFUSE_*` env is configured — when absent the
    /// gateway never reconstructs the body, so there is zero overhead and no
    /// behaviour change. When present, each gateway call fires a fire-and-forget
    /// trace beside the proxied request (never in its critical path).
    pub langfuse: Option<Arc<crate::langfuse::LangfuseClient>>,
    /// Pending "Sign in with Claude" OAuth logins (CCT-243), keyed by nonce.
    /// In-memory, TTL-bounded, scoped to the authenticated user. Entries are
    /// single-use (deleted on finish) and lazily swept on access.
    pub pending_oauth_logins: crate::routes::accounts::PendingOAuthLogins,
    /// Slow-refresh cache of per-account OAuth usage windows (CCT-306), keyed by
    /// account id. Anthropic's usage endpoint rate-limits per access token, so we
    /// serve a cached value and only re-fetch upstream past a TTL — the accounts
    /// view polls lazily and many clients share one cache entry per account.
    pub account_usage_cache: AccountUsageCache,
    /// Best-effort PR status cache the session classifier reads (GH-CLS-1,
    /// docs/github-integration.md §6.1). Core-owned and always present; the
    /// optional GitHub connector (feature `github`) pushes enriched check/review
    /// state into it. Empty when GitHub is absent — sessions still render and no
    /// `Review` bucket arises, so feature-off behaviour is unchanged. Only the
    /// `github`-feature route reads it today, so it is dead in a feature-off
    /// build (the field still exists so `AppState` has one shape either way).
    #[cfg_attr(not(feature = "github"), allow(dead_code))]
    pub pr_status_cache: cctui_proto::classifier::PrStatusCache,
    /// Sessions currently refused by the per-account soft limit (CCT-444), keyed
    /// by `session_id`. The gateway sets the entry when a passthrough is blocked
    /// and clears it on the next success (or on an explicit account switch), and
    /// only broadcasts the [`ServerEvent::SoftLimitReached`]/`SoftLimitCleared`
    /// on the actual transition so the worker's repeated Retry-After retries
    /// don't spam the WS stream.
    pub soft_limit_blocked: Arc<DashMap<String, ()>>,
    /// Orphan-token spam guard for the gateway, keyed by the SHA-256 fingerprint
    /// of the (unresolvable) session token. A worker whose session→account
    /// binding was lost retries `/gateway` indefinitely; each retry ran a DB
    /// lookup, so a single zombie could starve the connection pool and slow the
    /// whole server (including the webui). Once a fingerprint exceeds the spam
    /// threshold within the window we mark it blocked, and subsequent requests
    /// are dropped *before* any DB lookup until the block expires.
    pub gateway_orphan_spam: Arc<DashMap<String, OrphanSpam>>,
    /// Accounts whose upstream credentials the gateway has seen rejected
    /// (`needs_reauth`, CCT-512), keyed by account id. Mirrors the persisted
    /// `account_providers.needs_reauth` flag so the success path can clear it with a
    /// single DB write only on the actual transition — without this in-memory
    /// gate every successful passthrough would issue a conditional UPDATE.
    pub account_reauth: Arc<DashMap<Uuid, ()>>,
}

/// Sliding-window spam state for one orphan token fingerprint. See
/// [`AppState::gateway_orphan_spam`].
#[derive(Clone)]
pub struct OrphanSpam {
    /// Unresolved-401 count within the current window.
    pub count: u32,
    /// Start of the current counting window.
    pub window_start: std::time::Instant,
    /// When set and still in the future, requests for this fingerprint are
    /// dropped before any DB work.
    pub blocked_until: Option<std::time::Instant>,
}

/// A cached usage fetch: when it was fetched and the JSON payload (the raw
/// upstream usage windows). `None` payload means "fetched, but no usage" (e.g. a
/// non-anthropic account) — still cached so we don't re-hit upstream.
#[derive(Clone)]
pub struct CachedUsage {
    pub fetched_at: std::time::Instant,
    pub usage: Option<serde_json::Value>,
}

/// Per-account usage cache (CCT-306).
pub type AccountUsageCache = Arc<DashMap<Uuid, CachedUsage>>;
