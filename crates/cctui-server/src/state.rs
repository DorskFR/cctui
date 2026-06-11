use std::sync::Arc;

use cctui_proto::models::MachineLiveness;
use cctui_proto::ws::{DaemonFrameDown, ServerEvent};
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
    /// Pending "Sign in with Claude" OAuth logins (CCT-243), keyed by nonce.
    /// In-memory, TTL-bounded, scoped to the authenticated user. Entries are
    /// single-use (deleted on finish) and lazily swept on access.
    pub pending_oauth_logins: crate::routes::accounts::PendingOAuthLogins,
    /// Slow-refresh cache of per-account OAuth usage windows (CCT-306), keyed by
    /// account id. Anthropic's usage endpoint rate-limits per access token, so we
    /// serve a cached value and only re-fetch upstream past a TTL — the accounts
    /// view polls lazily and many clients share one cache entry per account.
    pub account_usage_cache: AccountUsageCache,
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
