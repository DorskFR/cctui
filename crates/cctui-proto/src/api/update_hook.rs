//! `/api/v1/version/self-update` — update-hook run state.

use serde::{Deserialize, Serialize};
#[cfg(feature = "ts")]
use ts_rs::TS;
use uuid::Uuid;

/// A hook run as the webui sees it.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct SelfUpdateRun {
    pub id: Uuid,
    pub machine_id: Uuid,
    /// Version this run is deploying.
    pub version: String,
    /// Version that was running when the run started.
    pub from_version: String,
    pub phase: crate::updatehook::UpdateHookPhase,
    /// Whether the run has finished, either way.
    pub done: bool,
    pub exit_code: Option<i32>,
    pub detail: String,
    /// Tail of the hook's output; `null` until a command has produced any.
    pub output_tail: Option<String>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Serialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct DaemonVersion {
    pub version: &'static str,
    pub git_hash: &'static str,
}
