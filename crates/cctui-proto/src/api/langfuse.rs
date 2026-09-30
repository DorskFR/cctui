//! Langfuse per-session usage rollup.

use serde::{Deserialize, Serialize};
#[cfg(feature = "ts")]
use ts_rs::TS;

/// Cost + `trace_count` are exact off the traces list; token classes are
/// best-effort — only populated when the deployment carries per-trace
/// `usageDetails` (legacy self-hosted trace lists often don't).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct LangfuseSessionUsage {
    pub cost_usd: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read: u64,
    pub trace_count: u64,
}
