use cctui_server_kernel::context::KernelContext;
use chrono::Utc;
use uuid::Uuid;

pub use crate::gateway::usage::*;

/// Append a fresh upstream reading to the credential's usage history, off the
/// request path: the caller has a payload to serve and the history is a
/// refinement (see [`crate::store::usage_samples`]).
///
/// Nothing to record for a
/// credential with no usage.
pub fn record_usage_samples(
    state: &impl KernelContext,
    provider_id: Uuid,
    usage: Option<&serde_json::Value>,
) {
    let Some(usage) = usage else {
        return;
    };
    let windows = cctui_proto::api::soft_limit::normalize_usage_windows(usage);
    if windows.is_empty() {
        return;
    }
    let pool = state.pool().clone();
    tokio::spawn(async move {
        crate::store::usage_samples::record(&pool, provider_id, &windows, Utc::now(), "poll").await;
    });
}
