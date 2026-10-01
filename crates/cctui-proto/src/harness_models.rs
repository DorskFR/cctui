//! Model and effort option lists per harness.
//!
//! There is no allowlist: these strings pass through verbatim and every picker
//! also accepts a free-text id. The lists exist so both clients offer the same
//! set, derived from the same catalog.

use serde::{Deserialize, Serialize};
#[cfg(feature = "ts")]
use ts_rs::TS;

use crate::codex_catalog::CodexModelCatalog;

/// Why a model is annotated in the picker. The client words it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ModelHint {
    /// The catalog view in use is older than the model needs, so it cannot be
    /// offered yet.
    Gated { version: String, current: String },
    /// The model needs a newer client than the one the catalog was read under,
    /// but the version in use is unknown.
    NeedsVersion { version: String },
}

/// One entry of a model picker.
///
/// `v` is the wire value; the empty string means "leave the harness its own
/// default".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct ModelOption {
    pub v: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub hint: Option<ModelHint>,
    #[serde(default)]
    pub disabled: bool,
}

impl ModelOption {
    fn plain(v: &str, label: &str) -> Self {
        Self { v: v.to_owned(), label: label.to_owned(), hint: None, disabled: false }
    }
}

/// Body of `GET /api/v1/models/{harness}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct HarnessModels {
    pub harness: String,
    pub models: Vec<ModelOption>,
    /// Effort levels, `""` (the harness default) first. Empty when the harness
    /// has no effort dial.
    pub efforts: Vec<String>,
}

fn default_option() -> ModelOption {
    ModelOption::plain("", "Default")
}

/// Offline fallback for codex, used only when no catalog is known.
///
/// No model slug is listed on purpose: the server fetches the catalog per
/// account, and free text covers a model no catalog has reached yet.
#[must_use]
pub fn codex_models() -> Vec<ModelOption> {
    vec![default_option()]
}

#[must_use]
pub fn codex_efforts() -> Vec<String> {
    ["", "low", "medium", "high", "xhigh", "max", "ultra"].map(String::from).to_vec()
}

#[must_use]
pub fn claude_models() -> Vec<ModelOption> {
    vec![
        default_option(),
        ModelOption::plain("haiku", "Haiku"),
        ModelOption::plain("sonnet", "Sonnet"),
        ModelOption::plain("opus", "Opus"),
        ModelOption::plain("fable", "Fable"),
    ]
}

#[must_use]
pub fn claude_efforts() -> Vec<String> {
    ["", "low", "medium", "high", "xhigh", "max"].map(String::from).to_vec()
}

/// Numeric semver compare, prerelease and build metadata ignored.
///
/// Unparseable input compares equal, so an odd version never disables a model.
#[must_use]
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    fn parts(v: &str) -> Option<Vec<u64>> {
        v.split(['-', '+']).next().unwrap_or_default().split('.').map(|n| n.parse().ok()).collect()
    }
    let (Some(x), Some(y)) = (parts(a), parts(b)) else {
        return Ordering::Equal;
    };
    for i in 0..x.len().max(y.len()) {
        let ord = x.get(i).copied().unwrap_or(0).cmp(&y.get(i).copied().unwrap_or(0));
        if ord != Ordering::Equal {
            return ord;
        }
    }
    Ordering::Equal
}

/// Options from a live catalog.
///
/// Hidden models are dropped and superseded ones suffixed, `Default` first. An
/// empty or absent catalog falls back to [`codex_models`].
#[must_use]
pub fn codex_models_for(catalog: Option<&CodexModelCatalog>) -> Vec<ModelOption> {
    let Some(catalog) = catalog.filter(|c| !c.models.is_empty()) else {
        return codex_models();
    };
    let current = catalog.client_version.clone().unwrap_or_default();
    let mut options = vec![default_option()];
    for model in &catalog.models {
        if model.hidden {
            continue;
        }
        let label = if model.upgrade.is_some() {
            format!("{} (superseded)", model.display_name)
        } else {
            model.display_name.clone()
        };
        let min = model.minimal_client_version.clone().unwrap_or_default();
        if min.is_empty() {
            options.push(ModelOption::plain(&model.id, &label));
            continue;
        }
        let gated = !current.is_empty() && compare_versions(&min, &current).is_gt();
        let hint = if gated {
            ModelHint::Gated { version: min, current: current.clone() }
        } else {
            ModelHint::NeedsVersion { version: min }
        };
        options.push(ModelOption { v: model.id.clone(), label, hint: Some(hint), disabled: gated });
    }
    options
}

/// Effort levels a model supports, `""` first.
///
/// An unknown model or an empty catalog falls back to the full static list.
#[must_use]
pub fn codex_efforts_for(catalog: Option<&CodexModelCatalog>, model_id: &str) -> Vec<String> {
    let Some(catalog) = catalog.filter(|c| !c.models.is_empty()) else {
        return codex_efforts();
    };
    let model = if model_id.is_empty() {
        catalog.models.iter().find(|m| m.is_default)
    } else {
        catalog.models.iter().find(|m| m.id == model_id)
    };
    let supported = model.map(|m| m.supported_efforts.clone()).unwrap_or_default();
    if supported.is_empty() {
        return codex_efforts();
    }
    std::iter::once(String::new()).chain(supported).collect()
}

/// Keeps a value the list does not know — a free-text id, or one remembered
/// from a spawn the catalog has since dropped — selectable by listing it.
#[must_use]
pub fn with_current_model(mut options: Vec<ModelOption>, current: &str) -> Vec<ModelOption> {
    if current.is_empty() || options.iter().any(|o| o.v == current) {
        return options;
    }
    options.push(ModelOption::plain(current, current));
    options
}

/// The lists for one harness.
///
/// `model` is the id already selected, so `efforts` are the ones it supports.
/// An unknown harness gets the claude shape, which is also what a free-text
/// picker needs.
#[must_use]
pub fn harness_models(
    harness: &str,
    catalog: Option<&CodexModelCatalog>,
    model: &str,
) -> HarnessModels {
    let (models, efforts) = match harness {
        "codex" => (codex_models_for(catalog), codex_efforts_for(catalog, model)),
        "opencode" => (vec![default_option()], Vec::new()),
        _ => (claude_models(), claude_efforts()),
    };
    HarnessModels { harness: harness.to_owned(), models, efforts }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codex_catalog::CodexModel;
    use std::cmp::Ordering;

    fn model(id: &str) -> CodexModel {
        CodexModel {
            id: id.into(),
            model: id.into(),
            display_name: id.to_uppercase(),
            description: String::new(),
            hidden: false,
            is_default: false,
            supported_efforts: vec![],
            default_effort: String::new(),
            input_modalities: vec![],
            upgrade: None,
            minimal_client_version: None,
        }
    }

    fn gated_catalog(client_version: Option<&str>) -> CodexModelCatalog {
        CodexModelCatalog {
            client_version: client_version.map(String::from),
            models: vec![
                model("gpt-5.5"),
                CodexModel {
                    minimal_client_version: Some("0.153.0".into()),
                    ..model("gpt-6-astra")
                },
                CodexModel { minimal_client_version: Some("0.999.0".into()), ..model("gpt-7") },
                CodexModel { hidden: true, ..model("codex-auto-review") },
            ],
        }
    }

    #[test]
    fn versions_order_numerically_and_ignore_prerelease_metadata() {
        assert_eq!(compare_versions("0.153.0", "0.156.1"), Ordering::Less);
        assert_eq!(compare_versions("0.156.1", "0.153.0"), Ordering::Greater);
        assert_eq!(compare_versions("0.156.1", "0.156.1"), Ordering::Equal);
        assert_eq!(compare_versions("0.156.1-rc.1", "0.156.1"), Ordering::Equal);
        assert_eq!(compare_versions("1.0", "1.0.0"), Ordering::Equal);
        assert_eq!(compare_versions("nonsense", "0.1.0"), Ordering::Equal);
    }

    #[test]
    fn the_static_codex_list_hardcodes_no_model_slug() {
        assert_eq!(codex_models().iter().map(|o| o.v.as_str()).collect::<Vec<_>>(), vec![""]);
    }

    #[test]
    fn an_empty_or_absent_catalog_falls_back_to_the_static_list() {
        assert_eq!(codex_models_for(None), codex_models());
        let empty = CodexModelCatalog { models: vec![], client_version: None };
        assert_eq!(codex_models_for(Some(&empty)), codex_models());
    }

    #[test]
    fn a_model_the_catalog_view_cannot_offer_is_disabled_and_hinted() {
        let catalog = gated_catalog(Some("0.156.1"));
        let options = codex_models_for(Some(&catalog));
        assert_eq!(
            options.iter().map(|o| o.v.as_str()).collect::<Vec<_>>(),
            vec!["", "gpt-5.5", "gpt-6-astra", "gpt-7"]
        );
        assert_eq!(options[1].hint, None);
        assert!(!options[1].disabled);
        assert!(!options[2].disabled);
        assert_eq!(options[2].hint, Some(ModelHint::NeedsVersion { version: "0.153.0".into() }));
        assert!(options[3].disabled);
        assert_eq!(
            options[3].hint,
            Some(ModelHint::Gated { version: "0.999.0".into(), current: "0.156.1".into() })
        );
    }

    #[test]
    fn an_unknown_client_version_hints_without_disabling() {
        let catalog = gated_catalog(None);
        let options = codex_models_for(Some(&catalog));
        assert!(!options[3].disabled);
        assert_eq!(options[3].hint, Some(ModelHint::NeedsVersion { version: "0.999.0".into() }));
    }

    #[test]
    fn hidden_models_are_dropped() {
        let catalog = gated_catalog(Some("0.156.1"));
        assert!(!codex_models_for(Some(&catalog)).iter().any(|o| o.v == "codex-auto-review"));
    }

    #[test]
    fn an_unknown_current_model_stays_selectable() {
        let known = vec![ModelOption::plain("", "Default"), ModelOption::plain("opus", "Opus")];
        assert_eq!(super::with_current_model(known.clone(), "opus").len(), 2, "already listed");
        assert_eq!(super::with_current_model(known.clone(), "").len(), 2, "the default is listed");

        let widened = super::with_current_model(known, "some-retired-model");
        assert_eq!(widened.len(), 3);
        let last = widened.last().expect("the added option");
        assert_eq!(last.v, "some-retired-model");
        assert_eq!(last.label, "some-retired-model");
        assert!(!last.disabled, "a remembered id must stay pickable");
    }

    #[test]
    fn superseded_models_are_labelled() {
        let catalog = CodexModelCatalog {
            client_version: None,
            models: vec![CodexModel { upgrade: Some("gpt-6".into()), ..model("gpt-5") }],
        };
        assert_eq!(codex_models_for(Some(&catalog))[1].label, "GPT-5 (superseded)");
    }

    #[test]
    fn efforts_come_from_the_model_and_fall_back_to_the_static_list() {
        let catalog = CodexModelCatalog {
            client_version: None,
            models: vec![
                CodexModel {
                    is_default: true,
                    supported_efforts: vec!["low".into(), "high".into()],
                    ..model("gpt-5.5")
                },
                model("gpt-7"),
            ],
        };
        assert_eq!(codex_efforts_for(Some(&catalog), ""), vec!["", "low", "high"]);
        assert_eq!(codex_efforts_for(Some(&catalog), "gpt-5.5"), vec!["", "low", "high"]);
        assert_eq!(codex_efforts_for(Some(&catalog), "gpt-7"), codex_efforts());
        assert_eq!(codex_efforts_for(Some(&catalog), "gpt-nonesuch"), codex_efforts());
        assert_eq!(codex_efforts_for(None, ""), codex_efforts());
    }

    #[test]
    fn an_unknown_harness_gets_the_claude_lists() {
        assert_eq!(harness_models("claude-code", None, "").models, claude_models());
        assert_eq!(harness_models("whatever", None, "").efforts, claude_efforts());
        assert_eq!(harness_models("codex", None, "").models, codex_models());
    }
}
