pub use cctui_server_core::gateway::credentials::*;

impl CredentialContext for crate::state::AppState {
    fn pool(&self) -> &sqlx::PgPool {
        &self.pool
    }
    fn http_client(&self) -> &reqwest::Client {
        &self.http_client
    }
    fn account_locks(
        &self,
    ) -> &dashmap::DashMap<uuid::Uuid, std::sync::Arc<tokio::sync::Mutex<()>>> {
        &self.account_locks
    }
    fn account_reauth(&self) -> &dashmap::DashMap<uuid::Uuid, ()> {
        &self.account_reauth
    }
    fn vault_key(&self) -> Vec<u8> {
        crate::crypto::vault_key()
    }
}
