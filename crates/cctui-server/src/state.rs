use std::sync::Arc;

use dashmap::DashMap;
#[cfg(test)]
use sqlx::PgPool;
use uuid::Uuid;

#[cfg(test)]
use cctui_server_dispatch::dispatchers::Registry as DispatcherRegistry;
use cctui_server_extras::skill_store::SkillStore;
#[cfg(test)]
use cctui_server_kernel::auth::AuthConfig;
#[cfg(test)]
use cctui_server_kernel::bus::Bus;
use cctui_server_kernel::config::Config;
use cctui_server_sessions::routes::permissions::SharedPermissionStore;

#[derive(Clone)]
pub struct AppState {
    pub core: crate::core_state::CoreState,
    pub config: Config,
    pub permission_store: SharedPermissionStore,
    /// `None` when no secure public URL is configured; passkey routes then
    /// answer "unavailable".
    pub passkeys: Option<Arc<cctui_server_accounts::relying_party::Passkeys>>,
    pub skills: Arc<SkillStore>,
    pub plugins: Arc<cctui_server_extras::plugins::PluginRegistry>,
    pub preview: Arc<cctui_server_extras::preview::Registry>,
    pub presence: Arc<cctui_server_extras::presence::PodIdentity>,
    /// Shared secret for pod-to-pod `/internal/bus/*` calls; `None` (no
    /// `CCTUI_POD_IP`) makes those endpoints refuse everything.
    pub internal_secret: Option<Arc<str>>,
    pub update_check: Arc<crate::update_check::UpdateCheck>,
    /// Serialises "Update" clicks: one self-update agent per release at a time.
    pub self_update: Arc<crate::routes::self_update::SelfUpdateGuard>,
    /// `None` unless `CCTUI_LANGFUSE_*` is configured.
    pub langfuse: Option<Arc<cctui_server_extras::langfuse::LangfuseClient>>,
    /// Has no feeder, so the `Review` bucket never surfaces.
    pub pr_status_cache: cctui_proto::classifier::PrStatusCache,
    /// Read-through cache over `codex_model_catalogs`, keyed by `machine_id`.
    pub codex_catalogs:
        Arc<DashMap<Uuid, cctui_server_sessions::routes::codex_models::CachedCatalog>>,
    /// Codex catalogs fetched with an account's own OAuth, keyed by provider
    /// credential; they outrank machine catalogs.
    pub codex_account_catalogs:
        Arc<DashMap<Uuid, cctui_server_sessions::routes::codex_models::CachedCatalog>>,
    /// Upstream gates the catalog by the `client_version` sent, so a stale value
    /// hides new models.
    pub codex_latest_version:
        Arc<std::sync::Mutex<Option<cctui_server_sessions::routes::codex_models::CachedVersion>>>,
    pub eviction_tracker: Arc<cctui_server_sessions::bandwidth_watch::EvictionTracker>,
    /// Rapid reconnects mean a daemon crashloop, which liveness never surfaces.
    pub connect_tracker: Arc<cctui_server_sessions::bandwidth_watch::ConnectTracker>,
    pub divergence_tracker: Arc<cctui_server_sessions::bandwidth_watch::DivergenceTracker>,
    /// Cached `instance_settings.upload_caps`, so the upload path never hits
    /// Postgres. Written on `PUT` and by a periodic reload for other replicas.
    pub upload_caps: Arc<std::sync::RwLock<cctui_server_sessions::uploads::UploadCaps>>,
}

#[cfg(test)]
impl AppState {
    #[must_use]
    pub fn for_test(pool: PgPool) -> Self {
        Self {
            core: crate::core_state::CoreState::new(
                pool.clone(),
                AuthConfig::new(vec![], pool),
                Bus::new(Box::new(cctui_server_kernel::bus::NoopTransport)),
                crate::build_http_client(),
                Arc::new(DispatcherRegistry::new()),
            ),
            config: Config::for_test(vec![]),
            permission_store: cctui_server_sessions::routes::permissions::PermissionStore::shared(),
            passkeys: None,
            skills: Arc::new(SkillStore::new(std::env::temp_dir().join("cctui-test-skills"))),
            plugins: Arc::new(cctui_server_extras::plugins::PluginRegistry::disabled()),
            preview: Arc::new(cctui_server_extras::preview::Registry::new(None, "", b"test")),
            presence: Arc::new(cctui_server_extras::presence::PodIdentity::from_env()),
            internal_secret: None,
            langfuse: None,
            pr_status_cache: cctui_proto::classifier::PrStatusCache::new(),
            codex_catalogs: Arc::new(DashMap::new()),
            codex_account_catalogs: Arc::new(DashMap::new()),
            codex_latest_version: Arc::new(std::sync::Mutex::new(None)),
            eviction_tracker: Arc::new(
                cctui_server_sessions::bandwidth_watch::EvictionTracker::default(),
            ),
            connect_tracker: Arc::new(
                cctui_server_sessions::bandwidth_watch::ConnectTracker::default(),
            ),
            divergence_tracker: Arc::new(
                cctui_server_sessions::bandwidth_watch::DivergenceTracker::default(),
            ),
            upload_caps: Arc::new(std::sync::RwLock::new(
                cctui_server_sessions::uploads::UploadCaps::default(),
            )),
            update_check: crate::update_check::UpdateCheck::shared(),
            self_update: Arc::new(crate::routes::self_update::SelfUpdateGuard::default()),
        }
    }
}

impl std::ops::Deref for AppState {
    type Target = crate::core_state::CoreState;
    fn deref(&self) -> &Self::Target {
        &self.core
    }
}
impl std::ops::DerefMut for AppState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.core
    }
}

impl axum::extract::FromRef<AppState> for crate::core_state::CoreState {
    fn from_ref(state: &AppState) -> Self {
        state.core.clone()
    }
}

impl axum::extract::FromRef<AppState> for cctui_server_extras::skills::SkillState {
    fn from_ref(state: &AppState) -> Self {
        Self { pool: state.pool.clone(), skills: state.skills.clone() }
    }
}

impl cctui_server_kernel::context::KernelContext for AppState {
    fn pool(&self) -> &sqlx::PgPool {
        &self.core.pool
    }
    fn bus(&self) -> &cctui_server_kernel::bus::Bus {
        &self.core.bus
    }
    fn auth_config(&self) -> &cctui_server_kernel::auth::AuthConfig {
        &self.core.auth_config
    }
    fn config(&self) -> &Config {
        &self.config
    }
    fn http_client(&self) -> &reqwest::Client {
        &self.core.http_client
    }
    fn registry(&self) -> &cctui_server_kernel::registry::SharedRegistry {
        &self.core.registry
    }
    fn pending_commands(&self) -> &DashMap<Uuid, cctui_server_kernel::commands::PendingCommand> {
        &self.core.pending_commands
    }
    fn command_waiters(&self) -> &cctui_server_kernel::commands::CommandWaiters {
        &self.core.command_waiters
    }
    fn spawn_capabilities(&self) -> &DashMap<String, cctui_proto::api::SpawnCapability> {
        &self.core.spawn_capabilities
    }
    fn session_usd_budgets(&self) -> &DashMap<String, f64> {
        &self.core.session_usd_budgets
    }
}
