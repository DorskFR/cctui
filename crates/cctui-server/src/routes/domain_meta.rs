//! `GET /api/v1/meta/domain` — the static domain tables both clients render
//! from: provider metadata, quota probes, end-reason tones, permission modes.
//!
//! Constant for the process, so it is cheap and cacheable; only the probe
//! registry is server-owned.

use axum::Json;
use cctui_proto::domain_meta::DomainMeta;

pub async fn get_domain_meta() -> Json<DomainMeta> {
    Json(DomainMeta::new(crate::usage_probe::picker_entries()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_probe_picker_leads_with_the_unmeasured_entry() {
        let Json(meta) = get_domain_meta().await;
        assert_eq!(meta.usage_probes[0].id, "");
        assert!(meta.usage_probes.iter().any(|p| p.id == "openrouter"));
        assert!(meta.usage_probes.iter().any(|p| p.id == "litellm"));
    }

    #[tokio::test]
    async fn the_provider_picker_order_is_served() {
        let Json(meta) = get_domain_meta().await;
        assert_eq!(meta.providers[0].id, "anthropic");
        assert_eq!(meta.providers[0].label, "Claude");
    }
}
