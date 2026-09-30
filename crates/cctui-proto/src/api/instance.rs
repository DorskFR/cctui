//! `/api/v1/admin/instance` — deployment name and self-update target.

use serde::{Deserialize, Serialize};
#[cfg(feature = "ts")]
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct InstanceUpdateRequest {
    /// New deployment name. Empty / whitespace-only clears it.
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct InstanceInfo {
    /// The deployment label, `null` when unset (the default).
    pub name: Option<String>,
}

/// Where the self-update agent runs. Stored under `instance_settings.self_update`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct SelfUpdateTarget {
    /// Enrolled machine (uuid) the update session is spawned on.
    pub machine_id: String,
    /// Working directory of that session; the deployment's checkout or
    /// operations folder, whatever the local instructions expect.
    pub working_dir: String,
    /// Adapter to run it under (`claude-code` / `codex`); `None` → claude-code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct SelfUpdateTargetRequest {
    /// `null` clears the stored target (the env fallback, if any, then applies).
    pub target: Option<SelfUpdateTarget>,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct SelfUpdateTargetInfo {
    /// The effective target: stored one first, else the env fallback, else
    /// `null` (the button then tells the admin to configure one).
    pub target: Option<SelfUpdateTarget>,
    /// `"settings"` when it comes from the admin form, `"env"` from
    /// `CCTUI_SELF_UPDATE_*`, `null` when unset.
    pub source: Option<&'static str>,
}
