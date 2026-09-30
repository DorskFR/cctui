//! `/api/v1/context` — reusable per-user context attached to a session at
//! spawn: durable memory notes and prompt templates.

use chrono::{DateTime, Utc};
use uuid::Uuid;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ContextItem {
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub id: Uuid,
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub user_id: Uuid,
    /// `memory` or `prompt`.
    pub kind: String,
    /// Slug, unique per `(user_id, kind)`; the stable reference a profile or a
    /// spawn request names.
    pub name: String,
    pub title: String,
    pub body: String,
    /// `user` | `machine` | `path` | `label`.
    pub scope: String,
    /// Machine id, working-dir prefix or label id. `None` for `user` scope.
    pub scope_ref: Option<String>,
    pub tags: Vec<String>,
    pub enabled: bool,
    pub version: i32,
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub created_at: DateTime<Utc>,
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub updated_at: DateTime<Utc>,
}

/// The editable half of an item.
#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
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

fn user_scope() -> String {
    "user".to_owned()
}

const fn yes() -> bool {
    true
}
