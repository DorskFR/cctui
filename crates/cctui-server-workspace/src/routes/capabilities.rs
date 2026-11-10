//! `GET /api/v1/capabilities` — which features and optional integrations this
//! server has, and whether each is operational. GitHub review is served by the separate
//! ghreview backend, which exposes its own `/v1/capabilities`.

use axum::Json;
use axum::extract::State;

pub use cctui_proto::api::capabilities::{CapabilitiesResponse, LangfuseCapability};

use crate::state::AppState;

/// `GET /api/v1/capabilities`.
pub async fn capabilities(State(state): State<AppState>) -> Json<CapabilitiesResponse> {
    let langfuse = match state.langfuse.as_ref() {
        Some(client) => LangfuseCapability {
            available: true,
            host: Some(client.host().to_string()),
            public_host: Some(client.public_host().to_string()),
            project_id: client.project_id().await,
        },
        None => {
            LangfuseCapability { available: false, host: None, public_host: None, project_id: None }
        }
    };

    Json(CapabilitiesResponse::full(langfuse, state.config.features))
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
    fn a_core_server_announces_only_always_on_features() {
        let core = crate::config::parse_features(include_str!(
            "../../../../deploy/features/core.toml"
        ))
        .unwrap()
        .set;
        let caps = CapabilitiesResponse::full(LangfuseCapability::default(), core);
        let v = serde_json::to_value(&caps).unwrap();
        assert_eq!(v["workspace"], true);
        assert_eq!(v["distribution"], "full");
        let features: Vec<&str> =
            v["features"].as_array().unwrap().iter().filter_map(|f| f.as_str()).collect();
        assert!(features.contains(&"dispatch") && features.contains(&"workspace"));
        for feature in cctui_proto::features::Feature::ALL {
            assert!(!features.contains(&feature.name()), "{feature} announced on core");
            assert!(!caps.has(feature), "{feature}");
        }
        assert_eq!(
            v["switchable"].as_array().unwrap().len(),
            cctui_proto::features::Feature::ALL.len()
        );
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
