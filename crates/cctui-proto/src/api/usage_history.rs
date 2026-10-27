//! `/api/v1/accounts/.../usage/*`: sampled usage, window closes and the per-reset ledger.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, rename = "UsageHistorySample"))]
pub struct HistorySample {
    pub window_key: String,
    pub utilization: f64,
    pub amount_usd: Option<f64>,
    pub resets_at: Option<DateTime<Utc>>,
    pub sampled_at: DateTime<Utc>,
    pub source: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct UsageHistory {
    pub account_id: Uuid,
    pub samples: Vec<HistorySample>,
}

#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, rename = "UsageWindowClose"))]
pub struct WindowClose {
    pub account_id: Uuid,
    pub window_key: String,
    pub resets_at: DateTime<Utc>,
    pub final_utilization: f64,
    pub wasted_pct: f64,
    pub closed_at: DateTime<Utc>,
    pub source: String,
}

/// Mean unused share of the closed instances of one window key.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct WastedSummary {
    pub window_key: String,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub windows: usize,
    pub mean_wasted_pct: f64,
}

#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, rename = "UsageWindowCloses"))]
pub struct WindowCloses {
    pub closes: Vec<WindowClose>,
    pub summary: Vec<WastedSummary>,
}

/// One reset of one window of one provider credential, with what went unused.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AccountLedgerRow {
    /// The provider credential, as `account_id` in `GET /accounts/usage`.
    pub account_id: Uuid,
    pub account_name: String,
    pub account_emoji: Option<String>,
    pub provider: String,
    pub window_key: String,
    pub window_label: String,
    pub resets_at: DateTime<Utc>,
    pub final_utilization: f64,
    pub unused_pct: f64,
    /// Unused share of the window times its length in hours.
    pub unused_capacity_hours: Option<f64>,
    pub amount_usd: Option<f64>,
    /// The last sample before the reset is too old to trust the final reading.
    pub sampling_gap: bool,
    /// Nothing was used in this window instance.
    pub idle: bool,
}

/// One credential's resets of one window over the requested range. Idle
/// closes are counted but left out of the mean and the capacity sum.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AccountLedgerTotal {
    pub account_id: Uuid,
    pub account_name: String,
    pub window_key: String,
    pub closes: u32,
    pub unused_pct_mean: Option<f64>,
    pub unused_capacity_hours: Option<f64>,
    pub idle_closes: u32,
}

/// `GET /api/v1/accounts/usage/ledger`: rows newest reset first.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AccountUsageLedger {
    pub rows: Vec<AccountLedgerRow>,
    pub totals: Vec<AccountLedgerTotal>,
}
