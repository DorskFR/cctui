use crate::state::AppState;
pub use cctui_server_core::gateway::ratelimit::*;
use uuid::Uuid;

/// Admit a request against the full host's shared provider window.
pub fn admit_request(state: &AppState, provider_id: Uuid, limits: &RateLimits) -> Result<(), i64> {
    let mut window = state.gateway_rate_windows.entry(provider_id).or_default();
    admit(&mut window, limits, std::time::Instant::now())
}
