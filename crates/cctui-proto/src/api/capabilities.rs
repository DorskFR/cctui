//! `GET /api/v1/capabilities` — supported server features and optional integrations.

use serde::{Deserialize, Serialize};
#[cfg(feature = "ts")]
use ts_rs::TS;

const EXECUTION_FEATURES: &[&str] = &[
    "identity",
    "accounts",
    "oauth",
    "enrollment",
    "dispatch",
    "machine_spawn",
    "gateway",
    "run_results",
    "cancellation",
    "failure_notifications",
    "enrolled_dispatchers",
];

fn full_distribution() -> String {
    "full".into()
}
const fn workspace_default() -> bool {
    true
}

/// Supported features describe the installed distribution, independently of
/// configured integrations and the caller's permissions. Missing flags from
/// older full servers preserve workspace support.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct CapabilitiesResponse {
    pub langfuse: LangfuseCapability,
    #[serde(default = "full_distribution")]
    pub distribution: String,
    #[serde(default = "workspace_default")]
    pub workspace: bool,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub experimental: bool,
}

impl CapabilitiesResponse {
    #[must_use]
    pub fn full(langfuse: LangfuseCapability) -> Self {
        Self {
            langfuse,
            distribution: full_distribution(),
            workspace: true,
            features: EXECUTION_FEATURES
                .iter()
                .copied()
                .chain(["workspace"])
                .map(str::to_owned)
                .collect(),
            experimental: false,
        }
    }

    #[must_use]
    pub fn core() -> Self {
        Self {
            langfuse: LangfuseCapability::default(),
            distribution: "core".into(),
            workspace: false,
            features: EXECUTION_FEATURES.iter().map(|feature| (*feature).to_owned()).collect(),
            experimental: true,
        }
    }
}

/// The Langfuse read integration's capability, as seen by the webui.
///
/// `available` gates every Langfuse UI element; `host` + `project_id` build the
/// `<host>/project/<id>/sessions/<uuid>` deep link. All `None` when the sink is unconfigured;
/// `project_id` alone `None` when the id could not be resolved.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct LangfuseCapability {
    pub available: bool,
    pub host: Option<String>,
    pub public_host: Option<String>,
    pub project_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn legacy_capabilities_preserve_full_workspace_support() {
        let legacy = json!({"langfuse":{"available":false,"host":null,"public_host":null,"project_id":null}});
        let caps: CapabilitiesResponse = serde_json::from_value(legacy).unwrap();
        let value = serde_json::to_value(caps).unwrap();
        assert_eq!(value["workspace"], true);
        assert_eq!(value["distribution"], "full");
        assert_eq!(value["experimental"], false);
    }

    #[test]
    fn core_workspace_restriction_survives_typed_decoding() {
        let core = json!({"langfuse":{"available":false,"host":null,"public_host":null,"project_id":null},"workspace":false,"distribution":"core","features":["dispatch"],"experimental":true});
        let caps: CapabilitiesResponse = serde_json::from_value(core).unwrap();
        let value = serde_json::to_value(caps).unwrap();
        assert_eq!(value["workspace"], false);
        assert_eq!(value["distribution"], "core");
        assert_eq!(value["features"], json!(["dispatch"]));
    }
}
