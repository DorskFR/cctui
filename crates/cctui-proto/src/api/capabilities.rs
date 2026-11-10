//! `GET /api/v1/capabilities` — supported server features and optional integrations.

use serde::{Deserialize, Serialize};
#[cfg(feature = "ts")]
use ts_rs::TS;

use crate::features::{Feature, FeatureSet};

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
    /// Every switchable feature this server knows, on or off. A feature absent
    /// from this list (an older server) is on.
    #[serde(default)]
    pub switchable: Vec<String>,
    #[serde(default)]
    pub experimental: bool,
}

impl CapabilitiesResponse {
    /// Always-on names, then the enabled switchable ones.
    #[must_use]
    pub fn full(langfuse: LangfuseCapability, enabled: FeatureSet) -> Self {
        Self {
            langfuse,
            distribution: full_distribution(),
            workspace: true,
            features: EXECUTION_FEATURES
                .iter()
                .copied()
                .filter(|name| Feature::parse(name).is_none())
                .chain(["workspace"])
                .chain(enabled.enabled().map(Feature::name))
                .map(str::to_owned)
                .collect(),
            switchable: Feature::ALL.into_iter().map(|f| f.name().to_owned()).collect(),
            experimental: false,
        }
    }

    /// Whether the server runs `feature`.
    #[must_use]
    pub fn has(&self, feature: Feature) -> bool {
        let name = feature.name();
        self.features.iter().any(|f| f == name) || !self.switchable.iter().any(|f| f == name)
    }

    #[must_use]
    pub fn core() -> Self {
        Self {
            langfuse: LangfuseCapability::default(),
            distribution: "core".into(),
            workspace: false,
            features: EXECUTION_FEATURES.iter().map(|feature| (*feature).to_owned()).collect(),
            switchable: Vec::new(),
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

    fn names(list: &[String]) -> Vec<&str> {
        list.iter().map(String::as_str).collect()
    }

    #[test]
    fn full_with_every_feature_on_lists_all_names() {
        let caps = CapabilitiesResponse::full(LangfuseCapability::default(), FeatureSet::all());
        let always = [
            "identity",
            "accounts",
            "oauth",
            "enrollment",
            "dispatch",
            "gateway",
            "run_results",
            "cancellation",
            "failure_notifications",
            "enrolled_dispatchers",
            "workspace",
        ];
        let switchable: Vec<&str> = Feature::ALL.into_iter().map(Feature::name).collect();
        let expected: Vec<&str> =
            always.iter().copied().chain(switchable.iter().copied()).collect();
        assert_eq!(names(&caps.features), expected);
        assert_eq!(names(&caps.switchable), switchable);
        assert!(caps.workspace);
        assert_eq!(caps.distribution, "full");
        assert!(Feature::ALL.iter().all(|f| caps.has(*f)));
    }

    #[test]
    fn full_with_every_feature_off_keeps_only_always_on_names() {
        let caps = CapabilitiesResponse::full(LangfuseCapability::default(), FeatureSet::none());
        assert!(!caps.features.iter().any(|f| f == "machine_spawn"));
        assert!(caps.features.iter().any(|f| f == "dispatch"));
        assert_eq!(caps.features.last().map(String::as_str), Some("workspace"));
        assert_eq!(caps.switchable.len(), Feature::ALL.len());
        assert!(Feature::ALL.iter().all(|f| !caps.has(*f)));
        let value = serde_json::to_value(&caps).unwrap();
        assert_eq!(value["workspace"], true);
        assert_eq!(value["distribution"], "full");
    }

    #[test]
    fn a_feature_unknown_to_an_older_server_counts_as_on() {
        let legacy = json!({"langfuse":{"available":false,"host":null,"public_host":null,"project_id":null},"features":["dispatch"]});
        let caps: CapabilitiesResponse = serde_json::from_value(legacy).unwrap();
        assert!(caps.switchable.is_empty());
        assert!(caps.has(Feature::Voice));
        let partial = CapabilitiesResponse { switchable: vec!["voice".into()], ..caps };
        assert!(!partial.has(Feature::Voice));
        assert!(partial.has(Feature::Rooms));
    }
}
