use crate::state::AppState;
use chrono::Utc;
use uuid::Uuid;

pub use cctui_server_core::gateway::usage::*;

impl UsageContext for AppState {
    fn usage_cache(&self) -> &dashmap::DashMap<Uuid, CachedUsage> {
        &self.account_usage_cache
    }

    fn probe_client(&self) -> &reqwest::Client {
        crate::outbound::upstream_client()
    }

    fn probe_url_permitted(&self, url: &str) -> Result<(), String> {
        crate::outbound::upstream_url_permitted(url).map_err(|e| e.to_string())
    }

    async fn usage_updated(&self, id: Uuid, provider: String, usage: Option<&serde_json::Value>) {
        crate::routes::accounts::render_and_broadcast_usage(self, id, provider, usage.cloned())
            .await;
        record_usage_samples(self, id, usage);
    }
}

/// Append a fresh upstream reading to the credential's usage history, off the
/// request path: the caller has a payload to serve and the history is a
/// refinement (see [`crate::store::usage_samples`]). Nothing to record for a
/// credential with no usage.
pub fn record_usage_samples(
    state: &AppState,
    provider_id: Uuid,
    usage: Option<&serde_json::Value>,
) {
    let Some(usage) = usage else {
        return;
    };
    let windows = crate::soft_limit::normalize_usage_windows(usage);
    if windows.is_empty() {
        return;
    }
    let pool = state.pool.clone();
    tokio::spawn(async move {
        crate::store::usage_samples::record(&pool, provider_id, &windows, Utc::now(), "poll").await;
    });
}
