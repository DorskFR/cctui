//! Per-account soft limit on the subscription usage windows (CCT-411).
//!
//! A cctui account (Anthropic OAuth subscription) is often shared with the user's
//! own interactive Claude Code and other workloads. Left unchecked, cctui's own
//! dispatched sessions can drive a window to 100% and rate-limit the human. The
//! soft limit caps cctui's *own* share of each window, backing off before it eats
//! the whole budget — while bypassing the cap for a window that is about to reset
//! anyway (no point hoarding it).
//!
//! This module is the pure decision helper. It is fed the cached usage payload
//! (Anthropic's free OAuth usage windows, CCT-306) and the per-account caps, and
//! returns Allow / Block. The gateway passthrough calls it after resolving the
//! account; it adds NO upstream fetch (it reuses the existing usage cache and
//! fails open when there is no cached value).

use chrono::{DateTime, Utc};

/// Per-account soft-limit configuration. Each field is independently optional;
/// `None` on a window's cap means "no soft limit on that window" (prior behaviour).
#[derive(Debug, Clone, Copy, Default)]
pub struct SoftLimits {
    /// Max % of the 5h window cctui will consume before refusing more inference.
    pub pct_5h: Option<i32>,
    /// Same for the 7d weekly window.
    pub pct_7d: Option<i32>,
    /// If a window's `resets_at` is within this many minutes, ignore its cap.
    pub bypass_minutes: Option<i32>,
}

impl SoftLimits {
    /// No cap configured on either window ⇒ nothing to evaluate (fast path).
    pub fn is_unset(&self) -> bool {
        self.pct_5h.is_none() && self.pct_7d.is_none()
    }
}

/// Outcome of evaluating an account's usage against its soft limits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Under cap, within the bypass window, no cap set, or no usage data — proxy.
    Allow,
    /// At/over a cap and not within the bypass window — refuse with a reason.
    Block {
        /// Seconds until the nearest blocking window resets (for `Retry-After`).
        retry_after_secs: i64,
        /// Human-readable reason surfaced to the worker/UI in the 429 body.
        reason: String,
    },
}

/// One window pulled out of the raw usage JSON.
struct Window {
    utilization: f64,
    resets_at: Option<DateTime<Utc>>,
}

fn parse_window(usage: &serde_json::Value, key: &str) -> Option<Window> {
    let w = usage.get(key)?;
    let utilization = w.get("utilization").and_then(serde_json::Value::as_f64)?;
    let resets_at = w
        .get("resets_at")
        .and_then(serde_json::Value::as_str)
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc));
    Some(Window { utilization, resets_at })
}

/// Decide whether to allow an inference request for an account, given its cached
/// usage and configured soft limits.
///
/// Fails open: missing usage, an unparseable window, or no caps ⇒ `Allow`. A
/// window blocks only when its utilization is at/above the cap AND its reset is
/// more than `bypass_minutes` away (or its reset time is unknown). When several
/// windows block, the reason names the nearest-resetting one and `retry_after`
/// is derived from that reset.
pub fn evaluate_soft_limit(
    usage: Option<&serde_json::Value>,
    caps: &SoftLimits,
    now: DateTime<Utc>,
) -> Decision {
    if caps.is_unset() {
        return Decision::Allow;
    }
    let Some(usage) = usage else { return Decision::Allow };
    let bypass = i64::from(caps.bypass_minutes.unwrap_or(0).max(0));

    // Collect every window that is currently blocking, with the seconds until it
    // resets (used both to apply the bypass window and to size Retry-After).
    let mut blocking: Vec<(i64, String)> = Vec::new();
    for (key, label, cap) in [("five_hour", "5h", caps.pct_5h), ("seven_day", "7d", caps.pct_7d)] {
        let Some(cap) = cap else { continue };
        let Some(win) = parse_window(usage, key) else { continue };
        if win.utilization < f64::from(cap) {
            continue;
        }
        // Seconds until reset; unknown reset ⇒ treat as far away (can't bypass).
        let secs_to_reset = win.resets_at.map(|r| (r - now).num_seconds()).unwrap_or(i64::MAX);
        // Within the bypass window (and not already past reset) ⇒ ignore this cap.
        if secs_to_reset > 0 && secs_to_reset <= bypass * 60 {
            continue;
        }
        let retry = secs_to_reset.max(1);
        let mins = (retry + 59) / 60;
        blocking.push((
            retry,
            format!(
                "cctui soft limit: {label} window at {}% (cap {cap}%), resets in {mins}m",
                win.utilization.round() as i64
            ),
        ));
    }

    match blocking.into_iter().min_by_key(|(secs, _)| *secs) {
        Some((retry_after_secs, reason)) => Decision::Block { retry_after_secs, reason },
        None => Decision::Allow,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-06-19T12:00:00Z").unwrap().with_timezone(&Utc)
    }

    fn usage(five: f64, five_reset: &str, seven: f64, seven_reset: &str) -> serde_json::Value {
        json!({
            "five_hour": { "utilization": five, "resets_at": five_reset },
            "seven_day": { "utilization": seven, "resets_at": seven_reset },
        })
    }

    #[test]
    fn no_caps_allows() {
        let u = usage(99.0, "2026-06-19T16:00:00Z", 99.0, "2026-06-26T00:00:00Z");
        assert_eq!(evaluate_soft_limit(Some(&u), &SoftLimits::default(), now()), Decision::Allow);
    }

    #[test]
    fn missing_usage_allows() {
        let caps = SoftLimits { pct_5h: Some(80), ..Default::default() };
        assert_eq!(evaluate_soft_limit(None, &caps, now()), Decision::Allow);
    }

    #[test]
    fn under_cap_allows() {
        let caps = SoftLimits { pct_5h: Some(80), ..Default::default() };
        let u = usage(50.0, "2026-06-19T16:00:00Z", 10.0, "2026-06-26T00:00:00Z");
        assert_eq!(evaluate_soft_limit(Some(&u), &caps, now()), Decision::Allow);
    }

    #[test]
    fn over_cap_blocks_with_reason_and_retry() {
        let caps = SoftLimits { pct_5h: Some(80), ..Default::default() };
        // 86% over cap 80, resets in 41 minutes (well outside any bypass).
        let u = usage(86.0, "2026-06-19T12:41:00Z", 10.0, "2026-06-26T00:00:00Z");
        match evaluate_soft_limit(Some(&u), &caps, now()) {
            Decision::Block { retry_after_secs, reason } => {
                assert_eq!(retry_after_secs, 41 * 60);
                assert_eq!(reason, "cctui soft limit: 5h window at 86% (cap 80%), resets in 41m");
            }
            d => panic!("expected block, got {d:?}"),
        }
    }

    #[test]
    fn within_bypass_window_allows() {
        // Over cap, but the window resets in 5 minutes and bypass is 10 ⇒ allow.
        let caps = SoftLimits { pct_5h: Some(80), bypass_minutes: Some(10), ..Default::default() };
        let u = usage(95.0, "2026-06-19T12:05:00Z", 10.0, "2026-06-26T00:00:00Z");
        assert_eq!(evaluate_soft_limit(Some(&u), &caps, now()), Decision::Allow);
    }

    #[test]
    fn outside_bypass_window_blocks() {
        let caps = SoftLimits { pct_5h: Some(80), bypass_minutes: Some(10), ..Default::default() };
        // Resets in 30m, bypass only 10m ⇒ still blocks.
        let u = usage(95.0, "2026-06-19T12:30:00Z", 10.0, "2026-06-26T00:00:00Z");
        assert!(matches!(evaluate_soft_limit(Some(&u), &caps, now()), Decision::Block { .. }));
    }

    #[test]
    fn nearest_resetting_blocking_window_wins() {
        // Both over cap; 7d resets sooner, so its reason + retry are reported.
        let caps = SoftLimits { pct_5h: Some(80), pct_7d: Some(70), ..Default::default() };
        let u = usage(90.0, "2026-06-19T16:00:00Z", 75.0, "2026-06-19T12:20:00Z");
        match evaluate_soft_limit(Some(&u), &caps, now()) {
            Decision::Block { retry_after_secs, reason } => {
                assert_eq!(retry_after_secs, 20 * 60);
                assert!(reason.contains("7d window"), "{reason}");
            }
            d => panic!("expected block, got {d:?}"),
        }
    }

    #[test]
    fn cap_only_on_unconfigured_window_allows() {
        // 7d is hot but only the 5h cap is configured ⇒ allow.
        let caps = SoftLimits { pct_5h: Some(80), ..Default::default() };
        let u = usage(10.0, "2026-06-19T16:00:00Z", 99.0, "2026-06-26T00:00:00Z");
        assert_eq!(evaluate_soft_limit(Some(&u), &caps, now()), Decision::Allow);
    }
}
