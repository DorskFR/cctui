pub use cctui_server_core::account_resolve::*;

impl AccountResolutionContext for crate::state::AppState {
    fn pool(&self) -> &sqlx::PgPool {
        &self.pool
    }

    fn cached_needs_reauth(&self, provider: uuid::Uuid) -> bool {
        self.account_reauth.contains_key(&provider)
    }

    async fn usage_for_soft_limit(&self, provider: uuid::Uuid) -> Option<serde_json::Value> {
        crate::routes::gateway::usage_for_soft_limit(self, provider).await
    }
}
