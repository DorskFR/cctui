//! Plugin manifests, catalog entries and admin settings.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
#[cfg(feature = "ts")]
use ts_rs::TS;

/// The env name a host-minted, `read`-scoped cctui token is exported under.
/// There is deliberately no scope field: the one cctui API a plugin's skill can
/// reach is its own backend proxy, which asks for `read`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct PluginHostToken {
    pub env: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct PluginPage {
    pub title: String,
    /// Tsumikit icon name for the nav entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct PluginSetting {
    pub key: String,
    pub label: String,
    pub env: String,
    #[serde(rename = "type")]
    pub kind: String,
}

/// One instance-level setting. `secret` values are sealed at rest and never
/// leave the server, not even to the admin who wrote them.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct PluginInstanceSetting {
    pub key: String,
    pub label: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub secret: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct PluginBackend {
    /// The `instanceSettings` key holding the upstream base URL.
    pub upstream_setting: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum PluginSource {
    Directory,
    Installed,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct PluginInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    /// Tsumikit icon name, when the manifest declares one.
    pub icon: Option<String>,
    /// `/plugins/<id>/<web>?v=<sha8>`, absent for skills-only plugins.
    pub web: Option<String>,
    /// Present when the plugin contributes a full page at `/apps/<id>`.
    pub page: Option<PluginPage>,
    /// Stylesheets to load with the module, as `/plugins/<id>/<path>?v=<sha8>`.
    pub styles: Vec<String>,
    pub skills: Vec<String>,
    /// From the caller's settings `plugins.enabled[id]`.
    pub enabled: bool,
    /// Per-user settings the plugin declares.
    pub settings: Vec<PluginSetting>,
    /// The caller's current values, by setting key (`plugins.config[id]`).
    pub config: BTreeMap<String, String>,
    /// Instance-level settings the admin owns, for display only.
    #[serde(rename = "instanceSettings")]
    pub instance_settings: Vec<PluginInstanceSetting>,
    /// Non-secret instance values, and only for a caller who enabled the
    /// plugin. Secrets are never included.
    #[serde(rename = "instanceSettingValues")]
    pub instance_setting_values: BTreeMap<String, String>,
    /// The plugin's backend is reachable at `/api/v1/plugins/<id>/backend/`.
    pub backend: bool,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct AdminPluginInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub source: PluginSource,
    /// The instance-wide toggle; directory plugins are always on.
    pub enabled: bool,
    /// Declarations for the admin settings form.
    pub instance_settings: Vec<PluginInstanceSetting>,
    /// The plugin declares a backend, so it needs an upstream and a secret.
    pub backend: bool,
    /// Set by an install that minted a fresh proxy secret; the only response
    /// that ever carries it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_secret: Option<String>,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct PluginInstallRequest {
    /// https URL of a `.tar.gz` plugin archive.
    pub url: String,
}

/// Install a published plugin by catalog id; the server resolves its url and
/// sha256 itself and never trusts client-supplied ones.
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct PluginCatalogInstallRequest {
    pub catalog: String,
}

/// A catalog entry as the admin UI sees it: the published metadata plus what
/// this instance has installed.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct CatalogPluginInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub homepage: Option<String>,
    /// `None` until this instance installs it.
    pub installed_version: Option<String>,
    /// The catalog version differs from the installed one.
    pub update_available: bool,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct PluginEnableRequest {
    pub enabled: bool,
}

/// The admin settings form for one plugin. Secret values are represented only
/// by `secrets_set`.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct PluginInstanceSettings {
    pub id: String,
    pub instance_settings: Vec<PluginInstanceSetting>,
    pub values: std::collections::BTreeMap<String, String>,
    pub secrets_set: std::collections::BTreeMap<String, bool>,
    /// The `instanceSettings` key the backend proxy reads the upstream from.
    pub backend_upstream_setting: Option<String>,
    /// Whether a proxy secret exists; its value is only ever shown at rotation.
    pub proxy_secret_set: bool,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct PluginInstanceSettingsRequest {
    /// Keys to write. An empty value clears the setting; omitted keys keep
    /// their current value.
    pub values: std::collections::BTreeMap<String, String>,
}

/// A rotated proxy secret, returned exactly once.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct PluginProxySecret {
    pub id: String,
    pub secret: String,
}

