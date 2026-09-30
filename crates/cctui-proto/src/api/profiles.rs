//! `/api/v1/profiles` — per-user spawn profiles.

use chrono::{DateTime, Utc};
use uuid::Uuid;

#[derive(Clone, Debug, serde::Serialize)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SessionProfile {
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub id: Uuid,
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub user_id: Uuid,
    pub name: String,
    pub harness: String,
    #[cfg_attr(feature = "ts", ts(type = "string | null"))]
    pub account_id: Option<Uuid>,
    #[cfg_attr(feature = "ts", ts(type = "string | null"))]
    pub pool_id: Option<Uuid>,
    pub no_account: bool,
    pub model_alias: Option<String>,
    pub effort: Option<String>,
    pub permission_mode: Option<String>,
    pub service_tier: Option<String>,
    /// Context items this profile pins, applied server-side at spawn.
    #[cfg_attr(feature = "ts", ts(type = "string[]"))]
    pub context_items: Vec<Uuid>,
    pub sort_order: i32,
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub created_at: DateTime<Utc>,
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub updated_at: DateTime<Utc>,
}

/// The knobs a profile carries. The account pick is at most one of
/// `account_id` / `pool_id` / `no_account`; none = Auto (the server elects one).
/// `None` model / effort / permission mode = the harness or account default.
#[derive(Clone, Debug, Default, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ProfileSpec {
    pub harness: String,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "string | null", optional))]
    pub account_id: Option<Uuid>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "string | null", optional))]
    pub pool_id: Option<Uuid>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "boolean", optional))]
    pub no_account: bool,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "string | null", optional))]
    pub model_alias: Option<String>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "string | null", optional))]
    pub effort: Option<String>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "string | null", optional))]
    pub permission_mode: Option<String>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "string | null", optional))]
    pub service_tier: Option<String>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "string[]", optional))]
    pub context_items: Vec<Uuid>,
}

#[derive(Debug, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct CreateProfileRequest {
    pub name: String,
    #[serde(flatten)]
    pub spec: ProfileSpec,
}

/// Every field optional; the spec, when present, replaces the whole kit (the
/// panel always holds the full one), so a cleared knob really clears.
#[derive(Debug, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct UpdateProfileRequest {
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub name: Option<String>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub spec: Option<ProfileSpec>,
}

#[derive(Debug, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ReorderProfilesRequest {
    #[cfg_attr(feature = "ts", ts(type = "string[]"))]
    pub ids: Vec<Uuid>,
}
