//! Full-host signal cleanup for shared account failover.
use crate::state::AppState;
pub use cctui_server_core::gateway::failover::*;
impl FailoverContext for AppState {
    fn provider_status(&self) -> &crate::provider_status::ProviderStatusCache {
        &self.provider_status
    }
    async fn clear_failover_signals(&self, session: &str) {
        super::clear_orphan_block_for_session(self, session).await;
        super::clear_soft_limit_block(self, session).await;
    }
}
