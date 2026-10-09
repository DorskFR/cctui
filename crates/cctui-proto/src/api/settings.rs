//! `/api/v1/settings` (per-user blob) and `/api/v1/admin/settings/*`
//! (admin-editable server settings).

use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(feature = "ts")]
use ts_rs::TS;

use crate::api::uploads::UploadCaps;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct SettingsPayload {
    pub version: i32,
    pub data: Value,
    /// Opaque marker of the stored row this copy was read from. A `PUT` carrying
    /// one is refused with `409` once another write has landed since.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub revision: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS))]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "ts", ts(export))]
pub enum SettingSource {
    Settings,
    Env,
    Default,
}

/// Default `CctuiAgent` limits; `null` fields are unset at that layer, and an
/// unset `max_children` or `max_tree_budget_usd` in `effective` means unlimited.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
#[allow(clippy::struct_field_names)]
pub struct SpawnDefaults {
    #[serde(default)]
    pub max_children: Option<u32>,
    #[serde(default)]
    pub max_depth: Option<u32>,
    #[serde(default)]
    pub max_tree_budget_usd: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
#[allow(clippy::struct_field_names)]
pub struct SpawnDefaultsSources {
    pub max_children: SettingSource,
    pub max_depth: SettingSource,
    pub max_tree_budget_usd: SettingSource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct SpawnDefaultsInfo {
    /// The values new sessions get; `null` = unlimited.
    pub effective: SpawnDefaults,
    pub sources: SpawnDefaultsSources,
    pub settings: SpawnDefaults,
    pub env: SpawnDefaults,
    pub defaults: SpawnDefaults,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct UpstreamHostsInfo {
    /// The saved, editable entries.
    pub hosts: Vec<String>,
    /// `settings` once a list has been saved, else `default`.
    pub source: SettingSource,
    /// `CCTUI_UPSTREAM_ALLOWED_HOSTS`, always allowed on top of `hosts`.
    pub env: Vec<String>,
    /// Always allowed on top of `hosts` (the `LiteLLM` endpoint).
    pub managed: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct UpstreamHostsRequest {
    /// `null` clears the saved list; env and managed hosts stay allowed.
    pub hosts: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct UploadCapsInfo {
    pub effective: UploadCaps,
    pub defaults: UploadCaps,
    /// `settings` once an admin saved a value, else `default`.
    pub source: SettingSource,
    /// The router's boot-time ceiling; a saved total cap must stay under it.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub body_limit_bytes: u64,
    /// Env var that sets `body_limit_bytes` (restart required).
    pub body_limit_env: &'static str,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct UploadCapsRequest {
    /// `null` clears the saved caps, restoring the built-in defaults.
    pub caps: Option<UploadCaps>,
}
