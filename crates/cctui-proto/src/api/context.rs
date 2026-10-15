//! `/api/v1/context` — reusable per-user context attached to a session:
//! always-on rules, durable memory notes and prompt templates.

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// When an item applies. Keys are AND-ed, the values of one key are OR-ed, and
/// an empty key matches anything, so an empty filter matches every launch.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ContextMatch {
    #[serde(default)]
    pub machines: Vec<String>,
    /// Absolute working-dir prefixes, matched on whole path components.
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub adapters: Vec<String>,
}

impl ContextMatch {
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.machines.is_empty()
            && self.paths.is_empty()
            && self.labels.is_empty()
            && self.adapters.is_empty()
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ContextItem {
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub id: Uuid,
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub user_id: Uuid,
    /// `memory`, `prompt` or `rule`.
    pub kind: String,
    /// Slug, unique per `(user_id, kind)`; the stable reference a profile or a
    /// spawn request names.
    pub name: String,
    pub title: String,
    pub body: String,
    /// `user` | `machine` | `path` | `label`. Superseded by `match`.
    pub scope: String,
    /// Machine id, working-dir prefix or label id. `None` for `user` scope.
    pub scope_ref: Option<String>,
    #[serde(rename = "match", default)]
    pub match_: ContextMatch,
    pub tags: Vec<String>,
    pub enabled: bool,
    pub version: i32,
    /// An agent's proposed replacement spec, awaiting the owner's approval.
    #[serde(default)]
    pub pending: Option<ContextItemSpec>,
    #[serde(default)]
    pub created_by_session: Option<String>,
    #[serde(default)]
    pub updated_by_session: Option<String>,
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub created_at: DateTime<Utc>,
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub updated_at: DateTime<Utc>,
}

/// The editable half of an item.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ContextItemSpec {
    pub kind: String,
    pub name: String,
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default = "user_scope")]
    pub scope: String,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "string | null", optional))]
    pub scope_ref: Option<String>,
    /// Wins over `scope`/`scope_ref` when non-empty.
    #[serde(rename = "match", default)]
    pub match_: ContextMatch,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct UpdateContextItemRequest {
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub spec: Option<ContextItemSpec>,
}

/// `GET /context/resolve`: what a launch into these coordinates gets.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ResolvedContextResponse {
    /// Enabled rules whose filter matches; always attached.
    pub rules: Vec<ContextItem>,
    /// Enabled memories whose filter matches; pre-checked, not forced.
    pub memories: Vec<ContextItem>,
}

/// `POST /sessions/{id}/context`.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AttachContextRequest {
    #[serde(default)]
    pub names: Vec<String>,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AttachContextResponse {
    pub attached: Vec<String>,
    /// Names that matched no memory of the caller's.
    pub unknown: Vec<String>,
}

/// One item a session received, at the version it received.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AttachedContextRef {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub title: String,
    pub version: i32,
}

/// `GET /sessions/{id}/context`.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SessionContextResponse {
    /// The rules the latest launch delivered.
    pub rules: Vec<AttachedContextRef>,
    /// The memories granted at spawn or attached since.
    pub memories: Vec<AttachedContextRef>,
}

/// The `CctuiContext` tool call the daemon relays to
/// `POST /daemon/sessions/{id}/context`.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ContextToolRequest {
    /// `list` | `show` | `save` | `attach`.
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(rename = "match", default, skip_serializing_if = "Option::is_none")]
    pub match_: Option<ContextMatch>,
    /// `create` | `replace` | `append`, for `save`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub names: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ContextToolEntry {
    pub kind: String,
    pub name: String,
    pub title: String,
    #[serde(rename = "match", default)]
    pub match_: ContextMatch,
    pub enabled: bool,
    pub version: i32,
    #[serde(default)]
    pub pending: bool,
    #[serde(default)]
    pub created_by_session: Option<String>,
    #[serde(default)]
    pub updated_by_session: Option<String>,
}

/// What `POST /daemon/sessions/{id}/context` answers. Validation failures
/// arrive as an HTTP error with the human API's message.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ContextToolResponse {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<ContextToolEntry>,
    /// `show` and `save`: the item as it now stands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<ContextToolEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// `save`: the change awaits the owner's approval.
    #[serde(default)]
    pub pending: bool,
    /// `attach`: the staged file the memories were written to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attached: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unknown: Vec<String>,
}

fn user_scope() -> String {
    "user".to_owned()
}

const fn yes() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_is_the_wire_name_and_every_key_defaults_to_any() {
        let spec: ContextItemSpec = serde_json::from_str(
            r#"{"kind":"rule","name":"wt","title":"W","match":{"adapters":["codex"]}}"#,
        )
        .expect("decodes");
        assert_eq!(spec.match_.adapters, ["codex"]);
        assert!(spec.match_.paths.is_empty());
        let wire = serde_json::to_value(&spec).expect("encodes");
        assert!(wire.get("match").is_some(), "{wire}");
        assert!(wire.get("match_").is_none(), "{wire}");

        let legacy: ContextItemSpec =
            serde_json::from_str(r#"{"kind":"memory","name":"n","title":"T"}"#).expect("decodes");
        assert!(legacy.match_.is_empty());
        assert_eq!(legacy.scope, "user");
    }
}
