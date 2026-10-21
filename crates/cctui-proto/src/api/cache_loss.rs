//! `/api/v1/stats/cache-loss` — daily prompt-cache loss attribution.

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct DailyCacheLoss {
    /// Local calendar day, `YYYY-MM-DD`.
    pub day: String,
    pub ttl_expired: f64,
    /// Always 0: the gateway forwards request bodies verbatim.
    pub gateway_rewrote_body: f64,
    /// A Fireworks turn missed its best-effort prompt cache.
    #[serde(default)]
    pub provider_miss: f64,
    pub unknown: f64,
    pub total: f64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub ttl_expired_tokens: u64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub gateway_rewrote_body_tokens: u64,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub provider_miss_tokens: u64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub unknown_tokens: u64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub lost_tokens: u64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub busts: u64,
}
