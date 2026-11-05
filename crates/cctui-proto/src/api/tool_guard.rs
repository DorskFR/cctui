//! `/api/v1/sessions/{id}/tool-guard` — lift the gateway tool guard for one session.

use serde::{Deserialize, Serialize};
#[cfg(feature = "ts")]
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct ToolGuardRequest {
    pub bypass: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct ToolGuardState {
    pub bypass: bool,
    /// Name of the user who lifted the guard.
    #[serde(default)]
    pub by: Option<String>,
    #[serde(default)]
    pub at: Option<chrono::DateTime<chrono::Utc>>,
    /// An account the session runs on has a policy that would scan its calls.
    #[serde(default)]
    pub policy_active: bool,
    /// The caller may toggle the bypass: an admin, or the owner of every
    /// account the session runs on.
    #[serde(default)]
    pub can_toggle: bool,
}
