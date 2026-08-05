//! `GET /api/v1/capabilities` — which optional integrations this server has,
//! and whether each is operational. GitHub review is served by the separate
//! ghreview backend, which exposes its own `/v1/capabilities`.

use axum::Json;
use axum::extract::State;
use serde::Serialize;
use ts_rs::TS;

use crate::state::AppState;

/// The capability envelope. One field per optional integration.
///
/// Self-hosted models are a per-account property surfaced by `GET /accounts`,
/// not a server-global list.
#[derive(Serialize, TS)]
#[ts(export)]
pub struct CapabilitiesResponse {
    pub langfuse: LangfuseCapability,
}

/// The Langfuse read integration's capability, as seen by the webui.
/// `available` gates every Langfuse UI element; `host` + `project_id` build the
/// `<host>/project/<id>/sessions/<uuid>` deep link. All `None` when the sink is
/// unconfigured; `project_id` alone `None` when the id could not be resolved.
#[derive(Serialize, TS)]
#[ts(export)]
pub struct LangfuseCapability {
    pub available: bool,
    pub host: Option<String>,
    pub public_host: Option<String>,
    pub project_id: Option<String>,
}

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

    Json(CapabilitiesResponse { langfuse })
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
}
