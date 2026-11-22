//! Execution caches and delivery state shared by `AppState` and the contract-test hosts.
use cctui_proto::models::MachineLiveness;
use cctui_server_accounts::gateway::limits::OrphanSpam;
use cctui_server_accounts::gateway::usage::AccountUsageCache;
use cctui_server_dispatch::dispatchers::Registry as DispatcherRegistry;
use cctui_server_kernel::auth::AuthConfig;
use cctui_server_kernel::bus::Bus;
use cctui_server_kernel::commands::{CommandWaiters, PendingCommand};
use cctui_server_kernel::registry::SharedRegistry;
use dashmap::DashMap;
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

/// Clones share execution caches and delivery state across request handlers.
#[derive(Clone)]
pub struct CoreState {
    pub pool: PgPool,
    pub registry: SharedRegistry,
    pub bus: Bus,
    pub auth_config: AuthConfig,
    pub dispatcher_liveness: Arc<DashMap<Uuid, MachineLiveness>>,
    pub dispatchers: Arc<DispatcherRegistry>,
    /// Last broadcast tier per machine, so [`cctui_proto::ws::ServerEvent::MachineLiveness`]
    /// fires only on a transition.
    pub machine_liveness: Arc<DashMap<Uuid, MachineLiveness>>,
    /// OAuth refresh tokens are single-use: held around read-expiry → refresh →
    /// persist so two sessions on one account never both refresh.
    pub account_locks: Arc<DashMap<Uuid, Arc<tokio::sync::Mutex<()>>>>,
    pub http_client: reqwest::Client,
    /// Upstream incident readings, refreshed by a background poller; never read
    /// from a status host on the request path.
    pub provider_status: Arc<cctui_server_accounts::provider_status::ProviderStatusCache>,
    /// Commands awaiting their daemon `CommandResult`, keyed by `command_id`.
    pub pending_commands: Arc<DashMap<Uuid, PendingCommand>>,
    /// Requests holding their reply a moment for the daemon's verdict, keyed
    /// by `command_id`; see [`cctui_server_kernel::commands::await_early_verdict`].
    pub command_waiters: CommandWaiters,
    /// Anthropic's usage endpoint rate-limits per access token, so fetches are
    /// cached per account and refreshed only past a TTL.
    pub account_usage_cache: AccountUsageCache,
    /// Keyed by the SHA-256 of an unresolvable session token: once over the
    /// threshold, requests are dropped before any DB lookup so a zombie worker
    /// cannot starve the pool.
    pub gateway_orphan_spam: Arc<DashMap<String, OrphanSpam>>,
    /// In-memory mirror of `account_providers.needs_reauth`, so the success
    /// path writes only on the transition.
    pub account_reauth: Arc<DashMap<Uuid, ()>>,
    pub machine_event_inserts: Arc<DashMap<Uuid, u64>>,
    /// Read-through cache over `session_spawn_capabilities`.
    pub spawn_capabilities: Arc<DashMap<String, cctui_proto::api::SpawnCapability>>,
    /// Read on the gateway hot path only while non-empty.
    pub session_usd_budgets: Arc<DashMap<String, f64>>,
    /// Machines whose daemon advertised `mark_acks`; only they may be sent
    /// [`cctui_proto::ws::DaemonFrameDown::TranscriptAck`].
    pub mark_ack_daemons: Arc<DashMap<Uuid, ()>>,
    /// Machines advertising targeted authentication recovery in their current connection.
    pub auth_recovery_daemons: Arc<DashMap<Uuid, ()>>,
    /// Rolling RPM/TPM windows, keyed by provider row id.
    pub gateway_rate_windows:
        Arc<DashMap<Uuid, cctui_server_accounts::gateway::ratelimit::RateWindow>>,
}
impl CoreState {
    #[must_use]
    pub fn new(
        pool: PgPool,
        auth_config: AuthConfig,
        bus: Bus,
        http_client: reqwest::Client,
        dispatchers: Arc<DispatcherRegistry>,
    ) -> Self {
        Self {
            pool,
            auth_config,
            bus,
            http_client,
            dispatchers,
            registry: cctui_server_kernel::registry::Registry::shared(),
            dispatcher_liveness: Arc::new(DashMap::new()),
            machine_liveness: Arc::new(DashMap::new()),
            account_locks: Arc::new(DashMap::new()),
            provider_status: cctui_server_accounts::provider_status::ProviderStatusCache::shared(),
            pending_commands: Arc::new(DashMap::new()),
            command_waiters: Arc::new(DashMap::new()),
            account_usage_cache: Arc::new(DashMap::new()),
            gateway_orphan_spam: Arc::new(DashMap::new()),
            account_reauth: Arc::new(DashMap::new()),
            machine_event_inserts: Arc::new(DashMap::new()),
            spawn_capabilities: Arc::new(DashMap::new()),
            session_usd_budgets: Arc::new(DashMap::new()),
            mark_ack_daemons: Arc::new(DashMap::new()),
            auth_recovery_daemons: Arc::new(DashMap::new()),
            gateway_rate_windows: Arc::new(DashMap::new()),
        }
    }
}

impl cctui_server_accounts::identity::IdentityContext for CoreState {
    fn pool(&self) -> &PgPool {
        &self.pool
    }
    fn bus(&self) -> &Bus {
        &self.bus
    }
    fn auth_config(&self) -> &AuthConfig {
        &self.auth_config
    }
}

impl cctui_server_dispatch::context::DispatchCore for CoreState {
    fn pool(&self) -> &PgPool {
        &self.pool
    }
    fn bus(&self) -> &Bus {
        &self.bus
    }
    fn auth_config(&self) -> &AuthConfig {
        &self.auth_config
    }
    fn http_client(&self) -> &reqwest::Client {
        &self.http_client
    }
    fn pending_commands(&self) -> &DashMap<Uuid, PendingCommand> {
        &self.pending_commands
    }
    fn command_waiters(&self) -> &CommandWaiters {
        &self.command_waiters
    }
    fn spawn_capabilities(&self) -> &DashMap<String, cctui_proto::api::SpawnCapability> {
        &self.spawn_capabilities
    }
    fn session_usd_budgets(&self) -> &DashMap<String, f64> {
        &self.session_usd_budgets
    }
    fn dispatcher_registry(&self) -> &Arc<DispatcherRegistry> {
        &self.dispatchers
    }
    fn dispatcher_liveness(&self) -> &DashMap<Uuid, MachineLiveness> {
        &self.dispatcher_liveness
    }
    fn machine_liveness(&self) -> &DashMap<Uuid, MachineLiveness> {
        &self.machine_liveness
    }
}

impl cctui_server_dispatch::targets::dispatcher::DispatcherContext for CoreState {
    async fn register_dispatcher_presence(&self, _: Uuid) {}
    async fn unregister_dispatcher_presence(&self, _: Uuid) {}
}

crate::impl_dispatch_account_store!(CoreState, pool);

impl cctui_server_dispatch::hooks::DispatchSessions for CoreState {
    async fn push_reconcile(
        &self,
        machine_id: Uuid,
    ) -> Result<(), cctui_server_kernel::bus::BusError> {
        cctui_server_sessions::targets::daemon::reconcile::push_reconcile(self, machine_id).await
    }
    async fn enabled_adapter_ids(&self, machine_id: Uuid) -> sqlx::Result<Vec<String>> {
        cctui_server_sessions::targets::daemon::reconcile::enabled_adapter_ids(self, machine_id)
            .await
    }
}

impl cctui_server_sessions::context::DaemonContext for CoreState {
    fn daemon_pool(&self) -> &PgPool {
        &self.pool
    }
    fn daemon_bus(&self) -> &Bus {
        &self.bus
    }
    fn mark_ack_daemons(&self) -> &Arc<DashMap<Uuid, ()>> {
        &self.mark_ack_daemons
    }
}
