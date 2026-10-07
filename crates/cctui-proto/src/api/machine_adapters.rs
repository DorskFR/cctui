//! `/api/v1/machines/{machine_id}/adapters` — which harnesses a machine runs.
//!
//! A harness with `default_enabled` runs everywhere unless a row disables it;
//! a default-off harness runs only where a row enables it.

use serde::{Deserialize, Serialize};
#[cfg(feature = "ts")]
use ts_rs::TS;

/// One harness as a machine sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct MachineAdapterInfo {
    pub adapter_id: String,
    /// Whether the daemon runs this harness after the next reconcile.
    pub enabled: bool,
    /// Per-machine adapter config the daemon receives; `{}` when nothing is set.
    #[cfg_attr(feature = "ts", ts(type = "Record<string, unknown>"))]
    pub config: serde_json::Value,
    /// Whether an `adapters_enabled` row exists; without one, `enabled` is the
    /// harness's `default_enabled`.
    pub pinned: bool,
    pub default_enabled: bool,
}

/// Body of `PUT /machines/{machine_id}/adapters/{adapter}`. An absent field
/// keeps the stored value; a fresh row starts enabled with `{}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct SetMachineAdapterRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(type = "Record<string, unknown> | null", optional))]
    pub config: Option<serde_json::Value>,
}
