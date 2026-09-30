//! `/api/v1/admin/privacy-scan` — transcript secret scanning jobs.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
#[cfg(feature = "ts")]
use ts_rs::TS;

#[derive(Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct RescrubRequest {
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub session_ids: Option<Vec<String>>,
    #[serde(default)]
    pub since: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct ScanSample {
    /// Any value the match carries is masked; a bare match is kept whole only
    /// when it cannot itself be a secret.
    pub text: String,
    pub context: String,
    pub value_follows: bool,
}

#[derive(Serialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct ScanCategory {
    pub category: String,
    pub count: i64,
    pub samples: Vec<ScanSample>,
    /// Most matches of this user pattern are bare identifiers with no value
    /// after them (`access_token` in prose, not a credential).
    pub identifier_warning: bool,
}

#[derive(Serialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
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
}
