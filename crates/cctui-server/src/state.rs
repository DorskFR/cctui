use std::sync::Arc;

use dashmap::DashMap;
#[cfg(test)]
use sqlx::PgPool;
use uuid::Uuid;

#[cfg(test)]
use crate::auth::AuthConfig;
#[cfg(test)]
use crate::bus::Bus;
use crate::config::Config;
#[cfg(test)]
use crate::dispatchers::Registry as DispatcherRegistry;
use crate::routes::permissions::SharedPermissionStore;
use crate::skill_store::SkillStore;

#[derive(Clone)]
pub struct AppState {
    pub core: cctui_server_core::state::CoreState,
    pub config: Config,
    pub permission_store: SharedPermissionStore,
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
    pub update_check: Arc<crate::update_check::UpdateCheck>,
    /// Serialises "Update" clicks: one self-update agent per release at a time.
    pub self_update: Arc<crate::routes::self_update::SelfUpdateGuard>,
    /// `None` unless `CCTUI_LANGFUSE_*` is configured.
    pub langfuse: Option<Arc<crate::langfuse::LangfuseClient>>,
    /// Single-use, TTL-bounded, swept lazily on access.
    pub pending_oauth_logins: crate::routes::accounts::PendingOAuthLogins,
    /// Has no feeder, so the `Review` bucket never surfaces.
    pub pr_status_cache: cctui_proto::classifier::PrStatusCache,
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
    /// Cached `instance_settings.upload_caps`, so the upload path never hits
    /// Postgres. Written on `PUT` and by a periodic reload for other replicas.
    pub upload_caps: Arc<std::sync::RwLock<crate::uploads::UploadCaps>>,
}

pub use cctui_server_core::gateway::usage::CachedUsage;

pub use cctui_server_core::commands::{
    CommandVerdict, FailedSpawnRow, resolve_verdict, track_command,
};

#[cfg(test)]
impl AppState {
    #[must_use]
    pub fn for_test(pool: PgPool) -> Self {
        Self {
            core: cctui_server_core::state::CoreState::new(
                pool.clone(),
                AuthConfig::new(vec![], pool),
                Bus::new(Box::new(crate::bus::NoopTransport)),
                crate::build_http_client(),
                Arc::new(DispatcherRegistry::new()),
            ),
            config: Config::for_test(vec![]),
            permission_store: crate::routes::permissions::PermissionStore::shared(),
            webauthn: None,
            skills: Arc::new(SkillStore::new(std::env::temp_dir().join("cctui-test-skills"))),
            plugins: Arc::new(crate::plugins::PluginRegistry::disabled()),
            preview: Arc::new(crate::preview::Registry::new(None, "", b"test")),
            presence: Arc::new(crate::presence::PodIdentity::from_env()),
            internal_secret: None,
            langfuse: None,
            pending_oauth_logins: Arc::new(DashMap::new()),
            pr_status_cache: cctui_proto::classifier::PrStatusCache::new(),
            codex_catalogs: Arc::new(DashMap::new()),
            codex_account_catalogs: Arc::new(DashMap::new()),
            codex_latest_version: Arc::new(std::sync::Mutex::new(None)),
            eviction_tracker: Arc::new(crate::bandwidth_watch::EvictionTracker::default()),
            connect_tracker: Arc::new(crate::bandwidth_watch::ConnectTracker::default()),
            divergence_tracker: Arc::new(crate::bandwidth_watch::DivergenceTracker::default()),
            upload_caps: Arc::new(std::sync::RwLock::new(crate::uploads::UploadCaps::default())),
            update_check: crate::update_check::UpdateCheck::shared(),
            self_update: Arc::new(crate::routes::self_update::SelfUpdateGuard::default()),
        }
    }
}

#[cfg(test)]
pub use cctui_server_core::commands::wait_for_verdict;

impl std::ops::Deref for AppState {
    type Target = cctui_server_core::state::CoreState;
    fn deref(&self) -> &Self::Target {
        &self.core
    }
}
impl std::ops::DerefMut for AppState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.core
    }
}

impl axum::extract::FromRef<AppState> for cctui_server_core::state::CoreState {
    fn from_ref(state: &AppState) -> Self {
        state.core.clone()
    }
}
