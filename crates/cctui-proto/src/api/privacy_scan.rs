//! `/api/v1/admin/privacy-scan` — transcript secret scanning jobs.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
#[cfg(test)]
use ts_rs::TS;

#[derive(Deserialize)]
#[cfg_attr(test, derive(TS), ts(export))]
pub struct RescrubRequest {
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub session_ids: Option<Vec<String>>,
    #[serde(default)]
    pub since: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(test, derive(TS), ts(export))]
pub struct ScanSample {
    /// Any value the match carries is masked; a bare match is kept whole only
    /// when it cannot itself be a secret.
    pub text: String,
    pub context: String,
    pub value_follows: bool,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(TS), ts(export))]
pub struct ScanCategory {
    pub category: String,
    pub count: i64,
    pub samples: Vec<ScanSample>,
    /// Most matches of this user pattern are bare identifiers with no value
    /// after them (`access_token` in prose, not a credential).
    pub identifier_warning: bool,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(TS), ts(export))]
pub struct PrivacyScanJob {
    pub id: String,
    /// `running` | `completed` | `cancelled` | `failed`.
    pub status: String,
    pub dry_run: bool,
    pub cancel_requested: bool,
    pub rows_total: Option<i64>,
    pub rows_scanned: i64,
    pub rows_changed: i64,
    pub substitutions: i64,
    pub by_category: BTreeMap<String, i64>,
    pub categories: Vec<ScanCategory>,
    pub error: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Absent when the local transcript opt-in was off as the job started.
    pub local_transcripts: Option<LocalTranscriptsScan>,
}

/// What one daemon found in its machine's harness transcripts. Counts and
/// masked samples only, never a value.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(test, derive(TS), ts(export))]
pub struct TranscriptAuditReport {
    pub dry_run: bool,
    pub files_scanned: i64,
    pub files_changed: i64,
    /// Files with findings that are never rewritten (codex rollouts).
    pub files_read_only: i64,
    /// Files a harness may still be writing: read, never rewritten.
    pub files_live: i64,
    pub files_busy: i64,
    pub files_failed: i64,
    pub lines_changed: i64,
    pub substitutions: i64,
    pub by_category: BTreeMap<String, i64>,
    pub samples: BTreeMap<String, Vec<ScanSample>>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(test, derive(TS), ts(export))]
pub enum MachineScanState {
    Pending,
    Scanned,
    NotScanned,
}

/// Why a machine was not scanned.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(test, derive(TS), ts(export))]
pub enum NotScannedReason {
    Offline,
    /// The daemon predates the transcript audit.
    Outdated,
    Timeout,
    Failed,
    Cancelled,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(test, derive(TS), ts(export))]
pub struct MachineTranscriptScan {
    pub machine_id: String,
    pub machine_name: String,
    pub state: MachineScanState,
    #[serde(default)]
    pub reason: Option<NotScannedReason>,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub report: Option<TranscriptAuditReport>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(TS), ts(export))]
pub struct LocalTranscriptsScan {
    pub machines: Vec<MachineTranscriptScan>,
    /// Every scanned machine's findings, summed per category.
    pub categories: Vec<ScanCategory>,
    pub substitutions: i64,
    pub files_changed: i64,
}
