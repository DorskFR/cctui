//! `/api/v1/accounts/{id}/usage-history` — sampled usage and window closes.

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

