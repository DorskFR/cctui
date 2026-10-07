//! The model and effort choices one agent session advertises.
//!
//! Sourced from `session/new` config options (category `model`,
//! `model_config` and `thought_level`) or the legacy `models` block, and kept
//! current from `config_option_update`.

use serde_json::Value;

use super::protocol::{Choice, NewSession, Selection};

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
            legacy.current = model.to_owned();
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
    }
}
