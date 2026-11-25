//! `/api/v1/context` — reusable per-user context attached to a session:
//! always-on rules, durable memory notes and prompt templates.

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Whose machines an item reaches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum ContextReach {
    /// Only machines the item's owner owns.
    #[default]
    Owner,
    /// Every user's machines. Only an admin may set it.
    AllUsers,
}

impl ContextReach {
    #[must_use]
    pub const fn is_owner(&self) -> bool {
        matches!(self, Self::Owner)
    }
}

/// When an item applies.
///
/// Keys are AND-ed, the values of one key are OR-ed, and an empty key matches
/// anything within the item's [`ContextReach`]. Dispatched workers only match
/// with `dispatch`, and only the owner's own dispatches.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export))]
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
    #[serde(default, skip_serializing_if = "ContextReach::is_owner")]
    #[cfg_attr(test, ts(as = "Option<ContextReach>", optional))]
    pub reach: ContextReach,
    /// Also reach workers the owner dispatches.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[cfg_attr(test, ts(as = "Option<bool>", optional))]
    pub dispatch: bool,
    /// Dispatcher names a dispatch must target; empty is any.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(test, ts(as = "Option<Vec<String>>", optional))]
    pub dispatchers: Vec<String>,
    /// Dispatch `payload.profile` values; empty is any.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(test, ts(as = "Option<Vec<String>>", optional))]
    pub profiles: Vec<String>,
}

impl ContextMatch {
    /// No key and no reach beyond the default.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.machines.is_empty()
            && self.paths.is_empty()
            && self.labels.is_empty()
            && self.adapters.is_empty()
            && self.reach.is_owner()
            && !self.dispatch
            && self.dispatchers.is_empty()
            && self.profiles.is_empty()
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export))]
pub struct ContextItem {
    #[cfg_attr(test, ts(type = "string"))]
    pub id: Uuid,
    #[cfg_attr(test, ts(type = "string"))]
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
    #[cfg_attr(test, ts(type = "string"))]
    pub created_at: DateTime<Utc>,
    #[cfg_attr(test, ts(type = "string"))]
    pub updated_at: DateTime<Utc>,
}

/// The editable half of an item.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export))]
pub struct ContextItemSpec {
    pub kind: String,
    pub name: String,
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default = "user_scope")]
    pub scope: String,
    #[serde(default)]
    #[cfg_attr(test, ts(type = "string | null", optional))]
    pub scope_ref: Option<String>,
    /// Wins over `scope`/`scope_ref` when non-empty.
    #[serde(rename = "match", default)]
    pub match_: ContextMatch,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
}

/// The same defaults a body omitting the fields decodes to.
impl Default for ContextItemSpec {
    fn default() -> Self {
        Self {
            kind: String::new(),
            name: String::new(),
            title: String::new(),
            body: String::new(),
            scope: user_scope(),
            scope_ref: None,
            match_: ContextMatch::default(),
            tags: Vec::new(),
            enabled: yes(),
        }
    }
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export))]
pub struct UpdateContextItemRequest {
    #[serde(default)]
    #[cfg_attr(test, ts(optional))]
    pub spec: Option<ContextItemSpec>,
}

/// `GET /context/resolve`: what a launch into these coordinates gets.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export))]
pub struct ResolvedContextResponse {
    /// Enabled rules whose filter matches; always attached.
    pub rules: Vec<ContextItem>,
    /// Enabled memories whose filter matches; pre-checked, not forced.
    pub memories: Vec<ContextItem>,
}

/// `POST /sessions/{id}/context`.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export))]
pub struct AttachContextRequest {
    #[serde(default)]
    pub names: Vec<String>,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export))]
pub struct AttachContextResponse {
    pub attached: Vec<String>,
    /// Names that matched no memory of the caller's.
    pub unknown: Vec<String>,
}

/// One item a session received, at the version it received.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export))]
pub struct AttachedContextRef {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub title: String,
    pub version: i32,
}

/// `GET /sessions/{id}/context`.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export))]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_summary: Option<String>,
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

/// What `PATCH /context` changes on every listed item. List keys merge into
/// the item's filter; a missing key is left alone.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export))]
pub struct ContextMatchLists {
    #[serde(default)]
    #[cfg_attr(test, ts(as = "Option<Vec<String>>", optional))]
    pub machines: Vec<String>,
    #[serde(default)]
    #[cfg_attr(test, ts(as = "Option<Vec<String>>", optional))]
    pub paths: Vec<String>,
    #[serde(default)]
    #[cfg_attr(test, ts(as = "Option<Vec<String>>", optional))]
    pub labels: Vec<String>,
    #[serde(default)]
    #[cfg_attr(test, ts(as = "Option<Vec<String>>", optional))]
    pub adapters: Vec<String>,
    #[serde(default)]
    #[cfg_attr(test, ts(as = "Option<Vec<String>>", optional))]
    pub dispatchers: Vec<String>,
    #[serde(default)]
    #[cfg_attr(test, ts(as = "Option<Vec<String>>", optional))]
    pub profiles: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export))]
pub struct ContextMatchSet {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub reach: Option<ContextReach>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub dispatch: Option<bool>,
    /// Empty `machines` before `add` applies.
    #[serde(default)]
    #[cfg_attr(test, ts(as = "Option<bool>", optional))]
    pub clear_machines: bool,
}

/// `PATCH /context`: one edit applied to many items, directly, with no
/// approval proposal.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export))]
pub struct BulkContextUpdate {
    #[cfg_attr(test, ts(type = "string[]"))]
    pub ids: Vec<Uuid>,
    #[serde(default)]
    #[cfg_attr(test, ts(as = "Option<ContextMatchLists>", optional))]
    pub add: ContextMatchLists,
    #[serde(default)]
    #[cfg_attr(test, ts(as = "Option<ContextMatchLists>", optional))]
    pub remove: ContextMatchLists,
    #[serde(default)]
    #[cfg_attr(test, ts(as = "Option<ContextMatchSet>", optional))]
    pub set: ContextMatchSet,
}

/// One item's outcome in a bulk edit: the item as it now stands, or why it
/// was left alone.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export))]
pub struct BulkContextOutcome {
    #[cfg_attr(test, ts(type = "string"))]
    pub id: Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub item: Option<ContextItem>,
    /// HTTP status the single-item edit would have answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(export))]
pub struct BulkContextResponse {
    pub outcomes: Vec<BulkContextOutcome>,
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

        let built = ContextItemSpec { kind: "memory".into(), ..ContextItemSpec::default() };
        assert_eq!((built.scope.as_str(), built.enabled), (legacy.scope.as_str(), legacy.enabled));
    }
}
