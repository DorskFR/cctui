//! Per-credential gateway knobs stored on the provider row.

pub const DEFAULT_STEP_PCT: u32 = 10;

/// Per-(account, provider) rate limits. Both optional; `None` ⇒ that dimension is
/// unlimited. Persisted as `{ "rpm": int?, "tpm": int? }` on the provider row.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct RateLimits {
    /// Max requests admitted per rolling 60s window.
    #[cfg_attr(feature = "ts", ts(type = "number | null", optional))]
    pub rpm: Option<u32>,
    /// Max tokens counted per rolling 60s window.
    #[cfg_attr(feature = "ts", ts(type = "number | null", optional))]
    pub tpm: Option<u64>,
}

impl RateLimits {
    /// Parse the stored `rate_limits_json` blob. A zero / negative / missing value
    /// leaves that dimension unlimited, so an operator clears a limit by zeroing it.
    pub fn from_json(value: Option<&serde_json::Value>) -> Self {
        let obj = value.and_then(serde_json::Value::as_object);
        let positive = |key: &str| {
            obj.and_then(|o| o.get(key)).and_then(serde_json::Value::as_u64).filter(|&n| n > 0)
        };
        Self { rpm: positive("rpm").and_then(|n| u32::try_from(n).ok()), tpm: positive("tpm") }
    }

    /// Nothing to enforce ⇒ the proxy skips the window entirely.
    pub const fn is_unset(&self) -> bool {
        self.rpm.is_none() && self.tpm.is_none()
    }
}

/// `{ "enabled": bool, "step_pct": int }` on the provider row. NULL ⇒ off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct UsageNotices {
    pub enabled: bool,
    pub step_pct: u32,
}

impl Default for UsageNotices {
    fn default() -> Self {
        Self { enabled: false, step_pct: DEFAULT_STEP_PCT }
    }
}

impl UsageNotices {
    pub fn from_json(value: Option<&serde_json::Value>) -> Self {
        let obj = value.and_then(serde_json::Value::as_object);
        let enabled = obj
            .and_then(|o| o.get("enabled"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let step_pct = obj
            .and_then(|o| o.get("step_pct"))
            .and_then(serde_json::Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| (1..=100).contains(n))
            .unwrap_or(DEFAULT_STEP_PCT);
        Self { enabled, step_pct }
    }

    /// Validate a PATCH/create payload into the stored blob; `Ok(None)` clears the
    /// column (off).
    pub fn build_json(
        value: Option<&serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, String> {
        let Some(v) = value.filter(|v| !v.is_null()) else { return Ok(None) };
        let Some(obj) = v.as_object() else { return Err("usage_notices must be an object".into()) };
        let enabled = match obj.get("enabled") {
            None | Some(serde_json::Value::Null) => false,
            Some(serde_json::Value::Bool(b)) => *b,
            Some(_) => return Err("usage_notices.enabled must be a boolean".into()),
        };
        let step_pct = match obj.get("step_pct") {
            None | Some(serde_json::Value::Null) => DEFAULT_STEP_PCT,
            Some(n) => n
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .filter(|n| (1..=100).contains(n))
                .ok_or_else(|| "usage_notices.step_pct must be an integer in 1..=100".to_owned())?,
        };
        if !enabled && step_pct == DEFAULT_STEP_PCT {
            return Ok(None);
        }
        Ok(Some(serde_json::json!({ "enabled": enabled, "step_pct": step_pct })))
    }
}
