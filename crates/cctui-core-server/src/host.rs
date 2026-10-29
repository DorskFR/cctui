//! Execution host resources and gateway adapters.
use cctui_server_core as core;
use core::account_resolve::AccountResolutionContext;
use core::gateway::credentials::{Account, CredentialContext};
use core::gateway::failover::FailoverContext;
use core::gateway::limits::{LimitContext, OrphanSpamMap};
use core::gateway::mint::MintContext;
use core::gateway::proxy::{ProxyContext, ResponseTee, TraceSink};
use core::gateway::ratelimit::RateWindow;
use core::gateway::usage::{CachedUsage, UsageContext};
use core::session_credentials::SessionCredentialContext;
use core::state::CoreState;
use dashmap::DashMap;
use serde_json::Value;
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

/// Clones share caches, target connections and the installed vault key.
#[derive(Clone)]
pub struct Host {
    pub core: CoreState,
    /// Serialize standalone registration, stop decisions and credential recovery.
    pub(crate) lifecycle: Arc<tokio::sync::Mutex<()>>,
    vault_key: Arc<Vec<u8>>,
    external_url: Arc<str>,
    github_token: Option<Arc<str>>,
    spawn_defaults: cctui_proto::api::settings::SpawnDefaults,
    oauth_logins: core::accounts::PendingOAuthLogins,
    pub(crate) ephemeral_machine_ttl_secs: u64,
}
impl Host {
    #[must_use]
    pub fn new(core: CoreState, vault_key: Vec<u8>, external_url: String) -> Self {
        Self {
            core,
            lifecycle: Arc::new(tokio::sync::Mutex::new(())),
            vault_key: Arc::new(vault_key),
            external_url: external_url.into(),
            github_token: None,
            spawn_defaults: cctui_proto::api::settings::SpawnDefaults::default(),
            oauth_logins: Arc::new(DashMap::new()),
            ephemeral_machine_ttl_secs: 2 * 60 * 60,
        }
    }
    #[must_use]
    pub fn with_github_token(mut self, token: Option<String>) -> Self {
        self.github_token = token.map(Into::into);
        self
    }
    #[must_use]
    pub const fn with_spawn_defaults(
        mut self,
        defaults: cctui_proto::api::settings::SpawnDefaults,
    ) -> Self {
        self.spawn_defaults = defaults;
        self
    }
    #[must_use]
    pub const fn with_worker_ttl_secs(mut self, ttl_secs: u64) -> Self {
        self.ephemeral_machine_ttl_secs = ttl_secs;
        self
    }
}
impl core::launch_credentials::LaunchCredentialsContext for Host {
    fn launch_state(&self) -> &CoreState {
        &self.core
    }
    async fn default_spawn_capability(&self) -> cctui_proto::api::SpawnCapability {
        core::spawn_defaults::capability(&self.pool, self.spawn_defaults).await
    }
}
impl std::ops::Deref for Host {
    type Target = CoreState;
    fn deref(&self) -> &Self::Target {
        &self.core
    }
}
impl axum::extract::FromRef<Host> for CoreState {
    fn from_ref(host: &Host) -> Self {
        host.core.clone()
    }
}
impl CredentialContext for Host {
    fn pool(&self) -> &PgPool {
        &self.core.pool
    }
    fn http_client(&self) -> &reqwest::Client {
        &self.core.http_client
    }
    fn account_locks(&self) -> &DashMap<Uuid, Arc<tokio::sync::Mutex<()>>> {
        &self.core.account_locks
    }
    fn account_reauth(&self) -> &DashMap<Uuid, ()> {
        &self.core.account_reauth
    }
    fn vault_key(&self) -> Vec<u8> {
        self.vault_key.as_ref().clone()
    }
}
impl MintContext for Host {
    fn pool(&self) -> &PgPool {
        &self.core.pool
    }
    fn vault_key(&self) -> Vec<u8> {
        self.vault_key.as_ref().clone()
    }
    fn external_url(&self) -> &str {
        &self.external_url
    }
    fn clear_orphan_fingerprint(&self, fp: &str) {
        self.core.gateway_orphan_spam.remove(fp);
    }
}
impl AccountResolutionContext for Host {
    fn pool(&self) -> &PgPool {
        &self.core.pool
    }
    fn cached_needs_reauth(&self, provider: Uuid) -> bool {
        self.core.account_reauth.contains_key(&provider)
    }
    async fn usage_for_soft_limit(&self, provider: Uuid) -> Option<Value> {
        core::gateway::usage::usage_for_soft_limit(self, provider).await
    }
}
impl UsageContext for Host {
    fn usage_cache(&self) -> &DashMap<Uuid, CachedUsage> {
        &self.core.account_usage_cache
    }
    fn probe_client(&self) -> &reqwest::Client {
        core::outbound::upstream_client()
    }
    fn probe_url_permitted(&self, url: &str) -> Result<(), String> {
        core::outbound::upstream_url_permitted(url).map_err(|e| e.to_string())
    }
    async fn usage_updated(&self, _: Uuid, _: String, _: Option<&Value>) {}
}
impl LimitContext for Host {
    fn limit_bus(&self) -> &core::bus::Bus {
        &self.core.bus
    }
    fn session_usd_budgets(&self) -> &DashMap<String, f64> {
        &self.core.session_usd_budgets
    }
    fn orphan_spam(&self) -> &OrphanSpamMap {
        &self.core.gateway_orphan_spam
    }
}
impl FailoverContext for Host {
    fn provider_status(&self) -> &core::provider_status::ProviderStatusCache {
        &self.core.provider_status
    }
    async fn clear_failover_signals(&self, session: &str) {
        core::gateway::limits::clear_orphan_block_for_session(self, session).await;
        core::gateway::limits::clear_soft_limit_block(self, session).await;
    }
}
impl ProxyContext for Host {
    fn rate_windows(&self) -> &Arc<DashMap<Uuid, RateWindow>> {
        &self.core.gateway_rate_windows
    }
    fn upstream_url_permitted(&self, url: &str) -> Result<(), core::outbound::OutboundUrlError> {
        core::outbound::upstream_url_permitted(url)
    }
    fn upstream_client(&self) -> &reqwest::Client {
        core::outbound::upstream_client()
    }
    fn trace_sink(&self) -> Option<Arc<dyn TraceSink>> {
        None
    }
    fn any_voice_watch(&self) -> bool {
        false
    }
    fn watched_session(&self, _: Option<String>) -> Option<String> {
        None
    }
    fn voice_from_request(&self, _: String, _: &Value) -> Option<Box<dyn ResponseTee>> {
        None
    }
    async fn deliver_usage_notice(&self, _: &Account, _: &str) {}
}
impl SessionCredentialContext for Host {
    fn token_pool(&self) -> &PgPool {
        &self.core.pool
    }
    fn spawn_capabilities(&self) -> &DashMap<String, cctui_proto::api::SpawnCapability> {
        &self.core.spawn_capabilities
    }
    fn session_budgets(&self) -> &DashMap<String, f64> {
        &self.core.session_usd_budgets
    }
    async fn after_execution_rebind(&self, _: &str, _: &str) -> Result<(), sqlx::Error> {
        Ok(())
    }
}

impl core::dispatch::DispatchContext for Host {
    fn dispatch_bus(&self) -> &core::bus::Bus {
        &self.core.bus
    }
    fn dispatchers(&self) -> &core::dispatchers::Registry {
        &self.core.dispatchers
    }
    fn notify(&self, _: core::dispatch::DispatchNotification) {}
    fn cache_spawn_capability(&self, session: &str, capability: cctui_proto::api::SpawnCapability) {
        self.core.spawn_capabilities.insert(session.to_owned(), capability);
    }
    async fn spawn_default_capability(&self) -> cctui_proto::api::SpawnCapability {
        core::launch_credentials::LaunchCredentialsContext::default_spawn_capability(self).await
    }
    async fn register_completion_webhook(
        &self,
        ctx: &core::auth::AuthContext,
        req: &cctui_proto::api::DispatchRequest,
        session: &str,
    ) {
        if let (Some(uid), Some(url)) =
            (ctx.owner_filter(), req.notify_url.as_deref().filter(|url| !url.trim().is_empty()))
        {
            let task_id =
                req.payload.get("task_id").and_then(serde_json::Value::as_str).unwrap_or(session);
            core::webhook::register(
                self,
                session,
                uid,
                url,
                req.notify_secret.as_deref().filter(|secret| !secret.trim().is_empty()),
                task_id,
            )
            .await;
        }
    }
    async fn remember_dispatch_context(
        &self,
        _: Option<Uuid>,
        _: &cctui_proto::api::DispatchRequest,
        _: &str,
    ) {
    }
}

impl core::accounts::AccountsContext for Host {
    fn account_core(&self) -> &CoreState {
        &self.core
    }
    fn oauth_logins(&self) -> &core::accounts::PendingOAuthLogins {
        &self.oauth_logins
    }
}

impl core::dispatch_liveness::DispatchLivenessContext for Host {
    async fn execution_finished(&self, session: &str) -> bool {
        sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM sessions WHERE id=$1 AND end_reason='completed')",
        )
        .bind(session)
        .fetch_one(&self.pool)
        .await
        .unwrap_or_else(|error| {
            tracing::warn!(%error, %session, "execution completion evidence read failed");
            false
        })
    }
}

impl core::targets::manifest::DistributionContext for Host {
    fn distribution_url(&self) -> &str {
        &self.external_url
    }
    fn github_token(&self) -> Option<&str> {
        self.github_token.as_deref()
    }
}
