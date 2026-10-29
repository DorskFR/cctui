//! Full-host resources for shared execution limits.
use crate::state::AppState;
pub use cctui_server_core::gateway::limits::*;
impl LimitContext for AppState {
    fn limit_bus(&self) -> &crate::bus::Bus {
        &self.bus
    }
    fn session_usd_budgets(&self) -> &dashmap::DashMap<String, f64> {
        &self.session_usd_budgets
    }
    fn orphan_spam(&self) -> &OrphanSpamMap {
        &self.gateway_orphan_spam
    }
}
