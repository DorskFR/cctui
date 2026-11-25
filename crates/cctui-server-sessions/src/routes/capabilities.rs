//! `GET /api/v1/capabilities` — which features and optional integrations this
//! server has, and whether each is operational.
//!
//! GitHub review is served by the separate
//! ghreview backend, which exposes its own `/v1/capabilities`.

use axum::Json;
use axum::extract::State;

pub use cctui_proto::api::capabilities::{CapabilitiesResponse, LangfuseCapability};

use crate::context::SessionsContext;

/// `GET /api/v1/capabilities`.
pub async fn capabilities<S: SessionsContext>(
    State(state): State<S>,
) -> Json<CapabilitiesResponse> {
    let langfuse = state.langfuse_capability().await;

    Json(CapabilitiesResponse::full(langfuse, state.config().features))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn langfuse_capability_off_hides_host_and_project() {
        let cap = LangfuseCapability {
            available: false,
            host: None,
            public_host: None,
            project_id: None,
        };
        let v = serde_json::to_value(cap).unwrap();
        assert_eq!(v["available"], false);
        assert!(v["host"].is_null());
        assert!(v["public_host"].is_null());
        assert!(v["project_id"].is_null());
    }

    #[test]
    fn langfuse_capability_on_carries_deep_link_parts() {
        let cap = LangfuseCapability {
            available: true,
            host: Some("https://lf.example".into()),
            public_host: Some("https://lf.public.example".into()),
            project_id: Some("proj_123".into()),
        };
        let v = serde_json::to_value(cap).unwrap();
        assert_eq!(v["available"], true);
        assert_eq!(v["host"], "https://lf.example");
        assert_eq!(v["public_host"], "https://lf.public.example");
        assert_eq!(v["project_id"], "proj_123");
    }

    #[test]
    fn a_full_server_announces_every_switchable_feature() {
        let caps = CapabilitiesResponse::full(
            LangfuseCapability::default(),
            cctui_proto::features::FeatureSet::all(),
        );
        for feature in cctui_proto::features::Feature::ALL {
            assert!(caps.features.iter().any(|f| f == feature.name()), "{feature}");
        }
    }
}
