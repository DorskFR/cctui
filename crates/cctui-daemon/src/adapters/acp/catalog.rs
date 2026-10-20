//! The model and effort choices one agent session advertises.
//!
//! Sourced from `session/new` config options (category `model`,
//! `model_config` and `thought_level`) or the legacy `models` block, kept
//! current from `config_option_update`, and reported to the server as the
//! machine's catalog for this harness so the picker can offer them.

use cctui_proto::adapter::AdapterEvent;
use cctui_proto::codex_catalog::{CodexModel, CodexModelCatalog};
use serde_json::Value;

use super::protocol::{Choice, NewSession, Selection};

const EFFORT_CATEGORIES: &[&str] = &["thought_level", "model_config"];

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Catalog {
    /// Config options as last reported, raw.
    options: Vec<Value>,
    /// The legacy `models` block, when the agent speaks it.
    legacy: Option<Selection>,
}

impl Catalog {
    #[must_use]
    pub fn from_session(session: &NewSession) -> Self {
        Self { options: session.config_options.clone(), legacy: session.models.clone() }
    }

    pub fn apply_config_options(&mut self, options: &[Value]) {
        self.options = options.to_vec();
    }

    /// The legacy `set_model` answered without a config option update.
    pub fn note_legacy_model(&mut self, model: &str) {
        if let Some(legacy) = self.legacy.as_mut() {
            model.clone_into(&mut legacy.current);
        }
    }

    fn option_in(&self, categories: &[&str]) -> Option<&Value> {
        self.options.iter().find(|o| {
            o.get("category").and_then(Value::as_str).is_some_and(|c| categories.contains(&c))
        })
    }

    /// The model the session runs on: the `model` config option's current
    /// value, else the legacy block's current model.
    #[must_use]
    pub fn current_model(&self) -> Option<String> {
        self.option_in(&["model"])
            .and_then(|o| o.get("currentValue").and_then(Value::as_str))
            .map(str::to_owned)
            .or_else(|| self.legacy.as_ref().map(|l| l.current.clone()))
    }

    /// The models the picker can offer, with the current one first.
    #[must_use]
    pub fn models(&self) -> Vec<Choice> {
        if let Some(option) = self.option_in(&["model"]) {
            return select_choices(option);
        }
        self.legacy.as_ref().map(|l| l.available.clone()).unwrap_or_default()
    }

    /// The effort/thinking option: a `thought_level` option, else a
    /// `model_config` select whose id or name says effort, thinking or
    /// reasoning.
    fn effort_option(&self) -> Option<&Value> {
        self.option_in(&EFFORT_CATEGORIES[..1]).or_else(|| {
            self.options.iter().find(|o| {
                o.get("category").and_then(Value::as_str) == Some(EFFORT_CATEGORIES[1])
                    && ["id", "name"].iter().any(|k| {
                        o.get(k).and_then(Value::as_str).is_some_and(|t| {
                            let t = t.to_ascii_lowercase();
                            t.contains("effort") || t.contains("think") || t.contains("reason")
                        })
                    })
            })
        })
    }

    /// The effort levels the session can switch between, empty when the
    /// agent exposes none.
    #[must_use]
    pub fn efforts(&self) -> Vec<Choice> {
        self.effort_option().map(select_choices).unwrap_or_default()
    }

    #[must_use]
    pub fn current_effort(&self) -> Option<String> {
        self.effort_option()
            .and_then(|o| o.get("currentValue").and_then(Value::as_str))
            .map(str::to_owned)
    }

    /// The catalog in the shape the server caches and the picker reads.
    #[must_use]
    pub fn catalog(&self) -> CodexModelCatalog {
        let current = self.current_model();
        let efforts: Vec<String> = self.efforts().into_iter().map(|c| c.id).collect();
        let default_effort = self.current_effort().unwrap_or_default();
        CodexModelCatalog {
            models: self
                .models()
                .into_iter()
                .map(|c| CodexModel {
                    display_name: if c.name.is_empty() { c.id.clone() } else { c.name },
                    description: c.description.unwrap_or_default(),
                    hidden: false,
                    is_default: current.as_deref() == Some(c.id.as_str()),
                    supported_efforts: efforts.clone(),
                    default_effort: default_effort.clone(),
                    input_modalities: Vec::new(),
                    upgrade: None,
                    minimal_client_version: None,
                    model: c.id.clone(),
                    id: c.id,
                })
                .collect(),
            client_version: None,
        }
    }

    /// The machine-scoped catalog event, when the agent advertises any model.
    #[must_use]
    pub fn event(&self, adapter_id: &str) -> Option<AdapterEvent> {
        let catalog = self.catalog();
        (!catalog.models.is_empty())
            .then(|| AdapterEvent::HarnessModels { adapter_id: adapter_id.to_owned(), catalog })
    }

    /// The requests a `SetModel` turns into, in order: `set_config_option`
    /// when the agent exposes the option, the legacy `set_model` for a model
    /// otherwise, and `Unsupported` when it exposes neither.
    pub fn set_model_requests(
        &self,
        session_id: &str,
        model: Option<&str>,
        effort: Option<&str>,
    ) -> anyhow::Result<Vec<(String, Value)>> {
        let mut out = Vec::new();
        if let Some(model) = model.filter(|m| !m.trim().is_empty()) {
            if let Some(id) = self.option_in(&["model"]).and_then(|o| o.get("id")?.as_str()) {
                out.push((
                    "session/set_config_option".to_owned(),
                    super::protocol::set_config_option_params(session_id, id, model),
                ));
            } else if self.legacy.is_some() {
                out.push((
                    "session/set_model".to_owned(),
                    super::protocol::set_model_params(session_id, model),
                ));
            } else {
                return Err(crate::adapter_runtime::Unsupported("set_model").into());
            }
        }
        if let Some(effort) = effort.filter(|e| !e.trim().is_empty()) {
            let Some(id) = self.effort_option().and_then(|o| o.get("id")?.as_str()) else {
                anyhow::bail!("this agent exposes no effort option");
            };
            out.push((
                "session/set_config_option".to_owned(),
                super::protocol::set_config_option_params(session_id, id, effort),
            ));
        }
        Ok(out)
    }
}

/// The selectable values of a `select` config option, grouped or not.
fn select_choices(option: &Value) -> Vec<Choice> {
    let Some(options) = option.get("options").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in options {
        if let Some(group) = entry.get("options").and_then(Value::as_array) {
            out.extend(group.iter().filter_map(choice));
        } else {
            out.extend(choice(entry));
        }
    }
    out
}

fn choice(v: &Value) -> Option<Choice> {
    Some(Choice {
        id: v.get("value").and_then(Value::as_str)?.to_owned(),
        name: v.get("name").and_then(Value::as_str).unwrap_or_default().to_owned(),
        description: v.get("description").and_then(Value::as_str).map(str::to_owned),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn model_option(current: &str) -> Value {
        json!({
            "id": "model",
            "name": "Model",
            "category": "model",
            "type": "select",
            "currentValue": current,
            "options": [
                { "value": "pro", "name": "Pro" },
                { "value": "flash", "name": "Flash", "description": "fast" },
            ],
        })
    }

    #[test]
    fn the_model_config_option_wins_over_the_legacy_block() {
        let session = NewSession {
            session_id: "s".into(),
            modes: None,
            models: Some(Selection {
                current: "legacy-model".into(),
                available: vec![Choice {
                    id: "legacy-model".into(),
                    name: "L".into(),
                    description: None,
                }],
            }),
            config_options: vec![model_option("flash")],
        };
        let catalog = Catalog::from_session(&session);
        assert_eq!(catalog.current_model().as_deref(), Some("flash"));
        assert_eq!(
            catalog.models().iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            ["pro", "flash"]
        );
    }

    #[test]
    fn legacy_models_are_the_fallback_and_follow_set_model() {
        let session = NewSession {
            session_id: "s".into(),
            modes: None,
            models: Some(Selection {
                current: "a".into(),
                available: vec![
                    Choice { id: "a".into(), name: "A".into(), description: None },
                    Choice { id: "b".into(), name: "B".into(), description: None },
                ],
            }),
            config_options: vec![],
        };
        let mut catalog = Catalog::from_session(&session);
        assert_eq!(catalog.current_model().as_deref(), Some("a"));
        catalog.note_legacy_model("b");
        assert_eq!(catalog.current_model().as_deref(), Some("b"));
        assert_eq!(catalog.models().len(), 2);
    }

    #[test]
    fn grouped_select_options_are_flattened() {
        let option = json!({
            "id": "model", "category": "model", "type": "select", "currentValue": "x",
            "options": [
                { "group": "g1", "name": "Group 1", "options": [{ "value": "x", "name": "X" }] },
                { "group": "g2", "name": "Group 2", "options": [{ "value": "y", "name": "Y" }] },
            ],
        });
        let mut catalog = Catalog::default();
        catalog.apply_config_options(&[option]);
        assert_eq!(catalog.models().iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["x", "y"]);
        assert_eq!(catalog.current_model().as_deref(), Some("x"));
    }

    #[test]
    fn an_empty_catalog_offers_nothing() {
        let catalog = Catalog::default();
        assert!(catalog.current_model().is_none());
        assert!(catalog.models().is_empty());
        assert!(catalog.event("gemini").is_none());
        let err = catalog.set_model_requests("s", Some("pro"), None).unwrap_err();
        assert!(err.is::<crate::adapter_runtime::Unsupported>(), "{err}");
        assert!(catalog.set_model_requests("s", None, None).unwrap().is_empty());
    }

    fn effort_option() -> Value {
        json!({
            "id": "thinking",
            "name": "Thinking level",
            "category": "thought_level",
            "type": "select",
            "currentValue": "medium",
            "options": [
                { "value": "low", "name": "Low" },
                { "value": "medium", "name": "Medium" },
                { "value": "high", "name": "High" },
            ],
        })
    }

    #[test]
    fn config_options_become_a_catalog_with_efforts_and_a_default() {
        let mut catalog = Catalog::default();
        catalog.apply_config_options(&[model_option("flash"), effort_option()]);
        let Some(AdapterEvent::HarnessModels { adapter_id, catalog: reported }) =
            catalog.event("gemini")
        else {
            panic!("no event")
        };
        assert_eq!(adapter_id, "gemini");
        let ids: Vec<&str> = reported.models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["pro", "flash"]);
        assert_eq!(reported.models[1].display_name, "Flash");
        assert_eq!(reported.models[1].description, "fast");
        assert!(reported.models[1].is_default && !reported.models[0].is_default);
        assert_eq!(reported.models[0].supported_efforts, ["low", "medium", "high"]);
        assert_eq!(reported.models[0].default_effort, "medium");
        assert_eq!(catalog.current_effort().as_deref(), Some("medium"));
    }

    #[test]
    fn set_model_prefers_config_options_then_the_legacy_pair() {
        let mut catalog = Catalog::default();
        catalog.apply_config_options(&[model_option("flash"), effort_option()]);
        let requests = catalog.set_model_requests("s", Some("pro"), Some("high")).unwrap();
        assert_eq!(
            requests,
            [
                (
                    "session/set_config_option".to_owned(),
                    json!({ "sessionId": "s", "configId": "model", "value": "pro" })
                ),
                (
                    "session/set_config_option".to_owned(),
                    json!({ "sessionId": "s", "configId": "thinking", "value": "high" })
                ),
            ]
        );

        let legacy = Catalog::from_session(&NewSession {
            session_id: "s".into(),
            modes: None,
            models: Some(Selection {
                current: "a".into(),
                available: vec![Choice { id: "a".into(), name: String::new(), description: None }],
            }),
            config_options: vec![],
        });
        let requests = legacy.set_model_requests("s", Some("b"), None).unwrap();
        assert_eq!(requests[0].0, "session/set_model");
        assert_eq!(requests[0].1, json!({ "sessionId": "s", "modelId": "b" }));
        let err = legacy.set_model_requests("s", None, Some("high")).unwrap_err();
        assert!(err.to_string().contains("no effort option"), "{err}");
        let reported = legacy.catalog();
        assert_eq!(reported.models[0].display_name, "a", "a nameless choice is labelled by id");
        assert!(reported.models[0].supported_efforts.is_empty());
    }

    #[test]
    fn a_model_config_select_about_reasoning_is_the_effort_option() {
        let mut catalog = Catalog::default();
        catalog.apply_config_options(&[json!({
            "id": "reasoning_effort",
            "name": "Reasoning effort",
            "category": "model_config",
            "type": "select",
            "currentValue": "low",
            "options": [{ "value": "low", "name": "Low" }, { "value": "max", "name": "Max" }],
        })]);
        assert_eq!(
            catalog.efforts().iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            ["low", "max"]
        );
        let mut other = Catalog::default();
        other.apply_config_options(&[json!({
            "id": "temperature", "name": "Temperature", "category": "model_config",
            "type": "select", "currentValue": "1", "options": [{ "value": "1", "name": "1" }],
        })]);
        assert!(other.efforts().is_empty());
    }
}
