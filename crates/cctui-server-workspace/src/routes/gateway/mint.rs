pub use cctui_server_core::gateway::mint::*;

use crate::state::AppState;

impl MintContext for AppState {
    fn pool(&self) -> &sqlx::PgPool {
        &self.pool
    }
    fn vault_key(&self) -> Vec<u8> {
        crate::crypto::vault_key()
    }
    fn external_url(&self) -> &str {
        &self.config.external_url
    }
    fn clear_orphan_fingerprint(&self, fingerprint: &str) {
        super::clear_orphan_fingerprint(&self.gateway_orphan_spam, fingerprint);
    }
}

pub use cctui_server_core::session_credentials::*;

impl SessionCredentialContext for AppState {
    fn token_pool(&self) -> &sqlx::PgPool {
        &self.pool
    }
    fn spawn_capabilities(&self) -> &dashmap::DashMap<String, cctui_proto::api::SpawnCapability> {
        &self.spawn_capabilities
    }
    fn session_budgets(&self) -> &dashmap::DashMap<String, f64> {
        &self.session_usd_budgets
    }
    async fn after_execution_rebind(&self, spawn: &str, session: &str) -> Result<(), sqlx::Error> {
        crate::store::tokens::rebind_attachments(&self.pool, spawn, session).await
    }
}
