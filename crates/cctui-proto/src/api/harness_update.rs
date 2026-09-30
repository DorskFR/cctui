//! `/api/v1/admin/harness-autoupdate` — per-machine harness update policy.

use serde::{Deserialize, Serialize};
#[cfg(feature = "ts")]
use ts_rs::TS;

#[derive(Serialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct MachineHarnessInfo {
    pub machine_id: String,
    pub name: String,
    /// `null` inherits the instance default.
    pub policy: Option<crate::harness::HarnessUpdatePolicy>,
    pub effective: crate::harness::HarnessUpdatePolicy,
    pub report: Option<crate::harness::HarnessReport>,
    #[cfg_attr(feature = "ts", ts(type = "string | null"))]
    pub report_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Serialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct HarnessAutoupdateInfo {
    pub instance: Option<crate::harness::HarnessUpdatePolicy>,
    pub machines: Vec<MachineHarnessInfo>,
}

#[derive(Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct HarnessPolicyRequest {
    /// `null` clears: the instance default falls back to off, a machine
    /// override falls back to the instance default.
    pub policy: Option<crate::harness::HarnessUpdatePolicy>,
}
