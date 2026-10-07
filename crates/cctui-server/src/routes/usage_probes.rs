//! `GET /api/v1/meta/usage-probes` — the quota-probe registry, the one domain
//! table only the server can enumerate. The closed tables (providers, end
//! reasons, permission modes) ship with the clients as parity fixtures.

use axum::Json;
use cctui_proto::provider::UsageProbeInfo;

pub async fn get_usage_probes() -> Json<Vec<UsageProbeInfo>> {
    Json(crate::usage_probe::picker_entries())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_probe_picker_leads_with_the_unmeasured_entry() {
        let Json(probes) = get_usage_probes().await;
        assert_eq!(probes[0].id, "");
        assert!(probes.iter().any(|p| p.id == "openrouter"));
        assert!(probes.iter().any(|p| p.id == "litellm"));
    }
}
