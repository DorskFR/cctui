use std::sync::Arc;
use std::time::{Duration, Instant};

use cctui_proto::models::MachineLiveness;
use dashmap::DashMap;
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::AuthConfig;
use crate::bus::Bus;
use crate::config::Config;
use crate::dispatchers::Registry as DispatcherRegistry;
use crate::registry::SharedRegistry;
use crate::routes::permissions::SharedPermissionStore;
use crate::skill_store::SkillStore;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub config: Config,
    pub registry: SharedRegistry,
    pub permission_store: SharedPermissionStore,
    pub bus: Bus,
    #[allow(dead_code)]
    pub auth_config: AuthConfig,
    /// `None` when no secure public URL is configured; passkey routes then
    /// answer "unavailable".
    pub webauthn: Option<Arc<webauthn_rs::Webauthn>>,
    pub skills: Arc<SkillStore>,
    pub plugins: Arc<crate::plugins::PluginRegistry>,
    pub preview: Arc<crate::preview::Registry>,
    pub presence: Arc<crate::presence::PodIdentity>,
    /// Shared secret for pod-to-pod `/internal/bus/*` calls; `None` (no
    /// `CCTUI_POD_IP`) makes those endpoints refuse everything.
    pub internal_secret: Option<Arc<str>>,
    pub dispatcher_liveness: Arc<DashMap<Uuid, MachineLiveness>>,
    pub dispatchers: Arc<DispatcherRegistry>,
    /// Last broadcast tier per machine, so [`ServerEvent::MachineLiveness`]
    /// fires only on a transition.
    pub machine_liveness: Arc<DashMap<Uuid, MachineLiveness>>,
    /// OAuth refresh tokens are single-use: held around read-expiry → refresh →
    /// persist so two sessions on one account never both refresh.
    pub account_locks: Arc<DashMap<Uuid, Arc<tokio::sync::Mutex<()>>>>,
    pub http_client: reqwest::Client,
    pub update_check: Arc<crate::update_check::UpdateCheck>,
    /// Serialises "Update" clicks: one self-update agent per release at a time.
    pub self_update: Arc<crate::routes::self_update::SelfUpdateGuard>,
    /// Commands awaiting their daemon `CommandResult`, keyed by `command_id`.
    pub pending_commands: Arc<DashMap<Uuid, PendingCommand>>,
    /// `None` unless `CCTUI_LANGFUSE_*` is configured.
    pub langfuse: Option<Arc<crate::langfuse::LangfuseClient>>,
    /// Single-use, TTL-bounded, swept lazily on access.
    pub pending_oauth_logins: crate::routes::accounts::PendingOAuthLogins,
    /// Anthropic's usage endpoint rate-limits per access token, so fetches are
    /// cached per account and refreshed only past a TTL.
    pub account_usage_cache: AccountUsageCache,
    /// Has no feeder, so the `Review` bucket never surfaces.
    pub pr_status_cache: cctui_proto::classifier::PrStatusCache,
    /// Keyed by the SHA-256 of an unresolvable session token: once over the
    /// threshold, requests are dropped before any DB lookup so a zombie worker
    /// cannot starve the pool.
    pub gateway_orphan_spam: Arc<DashMap<String, OrphanSpam>>,
    /// In-memory mirror of `account_providers.needs_reauth`, so the success
    /// path writes only on the transition.
    pub account_reauth: Arc<DashMap<Uuid, ()>>,
    /// Read-through cache over `codex_model_catalogs`, keyed by `machine_id`.
    pub codex_catalogs: Arc<DashMap<Uuid, crate::routes::codex_models::CachedCatalog>>,
    /// Codex catalogs fetched with an account's own OAuth, keyed by provider
    /// credential; they outrank machine catalogs.
    pub codex_account_catalogs: Arc<DashMap<Uuid, crate::routes::codex_models::CachedCatalog>>,
    /// Upstream gates the catalog by the `client_version` sent, so a stale value
    /// hides new models.
    pub codex_latest_version:
        Arc<std::sync::Mutex<Option<crate::routes::codex_models::CachedVersion>>>,
    pub eviction_tracker: Arc<crate::bandwidth_watch::EvictionTracker>,
    /// Rapid reconnects mean a daemon crashloop, which liveness never surfaces.
    pub connect_tracker: Arc<crate::bandwidth_watch::ConnectTracker>,
    pub divergence_tracker: Arc<crate::bandwidth_watch::DivergenceTracker>,
    pub machine_event_inserts: Arc<DashMap<Uuid, u64>>,
    /// Read-through cache over `session_spawn_capabilities`.
    pub spawn_capabilities: Arc<DashMap<String, cctui_proto::api::SpawnCapability>>,
    /// Read on the gateway hot path only while non-empty.
    pub session_usd_budgets: Arc<DashMap<String, f64>>,
    /// Rolling RPM/TPM windows, keyed by provider row id.
    pub gateway_rate_windows: Arc<DashMap<Uuid, crate::routes::gateway::RateWindow>>,
}

#[derive(Clone)]
pub struct OrphanSpam {
    pub count: u32,
    pub window_start: std::time::Instant,
    pub blocked_until: Option<std::time::Instant>,
}

/// A `None` payload means "fetched, but no usage" and is cached too.
#[derive(Clone)]
pub struct CachedUsage {
    pub fetched_at: std::time::Instant,
    pub usage: Option<serde_json::Value>,
}

pub type AccountUsageCache = Arc<DashMap<Uuid, CachedUsage>>;

/// A failed spawn's `sessions` row: the daemon never registers a session that
/// failed to start, so its identity survives only here.
#[derive(Debug, Clone)]
pub struct FailedSpawnRow {
    pub session_id: String,
    pub machine_id: Uuid,
    pub user_id: Uuid,
    pub adapter_id: String,
    pub working_dir: String,
    pub name: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PendingCommand {
    pub session_id: Option<String>,
    /// Consumed only when the spawn fails.
    pub spawn: Option<FailedSpawnRow>,
    pub at: Instant,
}

pub const PENDING_COMMAND_TTL: Duration = Duration::from_mins(10);

pub fn track_command(
    map: &DashMap<Uuid, PendingCommand>,
    command_id: Uuid,
    session_id: Option<String>,
    spawn: Option<FailedSpawnRow>,
) {
    let now = Instant::now();
    map.retain(|_, c| now.duration_since(c.at) < PENDING_COMMAND_TTL);
    map.insert(command_id, PendingCommand { session_id, spawn, at: now });
}

#[cfg(test)]
impl AppState {
    #[must_use]
    pub fn for_test(pool: PgPool) -> Self {
        Self {
            config: Config::for_test(vec![]),
            registry: crate::registry::Registry::shared(),
            permission_store: crate::routes::permissions::PermissionStore::shared(),
            bus: Bus::new(Box::new(crate::bus::NoopTransport)),
            auth_config: AuthConfig::new(vec![], pool.clone()),
            webauthn: None,
            skills: Arc::new(SkillStore::new(std::env::temp_dir().join("cctui-test-skills"))),
            plugins: Arc::new(crate::plugins::PluginRegistry::disabled()),
            preview: Arc::new(crate::preview::Registry::new(None, "", b"test")),
            presence: Arc::new(crate::presence::PodIdentity::from_env()),
            internal_secret: None,
            dispatcher_liveness: Arc::new(DashMap::new()),
            dispatchers: Arc::new(DispatcherRegistry::new()),
            machine_liveness: Arc::new(DashMap::new()),
            account_locks: Arc::new(DashMap::new()),
            http_client: reqwest::Client::new(),
            langfuse: None,
            pending_oauth_logins: Arc::new(DashMap::new()),
            account_usage_cache: Arc::new(DashMap::new()),
            pr_status_cache: cctui_proto::classifier::PrStatusCache::new(),
            gateway_orphan_spam: Arc::new(DashMap::new()),
            account_reauth: Arc::new(DashMap::new()),
            codex_catalogs: Arc::new(DashMap::new()),
            codex_account_catalogs: Arc::new(DashMap::new()),
            codex_latest_version: Arc::new(std::sync::Mutex::new(None)),
            eviction_tracker: Arc::new(crate::bandwidth_watch::EvictionTracker::default()),
            connect_tracker: Arc::new(crate::bandwidth_watch::ConnectTracker::default()),
            divergence_tracker: Arc::new(crate::bandwidth_watch::DivergenceTracker::default()),
            machine_event_inserts: Arc::new(DashMap::new()),
            spawn_capabilities: Arc::new(DashMap::new()),
            session_usd_budgets: Arc::new(DashMap::new()),
            gateway_rate_windows: Arc::new(DashMap::new()),
            update_check: crate::update_check::UpdateCheck::shared(),
            self_update: Arc::new(crate::routes::self_update::SelfUpdateGuard::default()),
            pending_commands: Arc::new(DashMap::new()),
            pool,
        }
    }
}
