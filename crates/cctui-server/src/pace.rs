//! Burn-rate math for a usage window: how far into the window we are, what a
//! linear spend would have consumed by now, and when the wall (100%) lands if
//! the current rate holds. Pure functions; the usage routes attach the result
//! to every window and the pace-limit enforcement reuses the same numbers.

use chrono::{DateTime, Duration, Utc};

use crate::soft_limit::{KEY_SESSION, KEY_USD_5H, KEY_USD_7D, KEY_WEEKLY_ALL, WEEKLY_MODEL_PREFIX};

/// Floor on `expected_pct` when forming the ratio so the first minute of a
/// window never divides by ~0.
const MIN_EXPECTED_PCT: f64 = 1.0;

/// Pace of one window relative to its linear budget.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct Pace {
    /// 0..1: share of the window already elapsed.
    pub elapsed_fraction: f64,
    /// Utilization a perfectly even spend would show now (`elapsed_fraction * 100`).
    pub expected_pct: f64,
    /// `utilization / expected_pct` — < 1 under pace, > 1 burning too fast.
    pub ratio: f64,
    /// When utilization reaches 100% at the current rate; `None` when idle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projected_wall_at: Option<DateTime<Utc>>,
}

/// An earlier utilization reading of the same window, for a two-point rate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub at: DateTime<Utc>,
    pub utilization: f64,
}

/// Length of a canonical window, or `None` for one that never resets
/// (a per-session dollar budget) or an unknown key.
pub fn window_duration(key: &str) -> Option<Duration> {
    if key == KEY_SESSION || key == KEY_USD_5H {
        Some(Duration::hours(5))
    } else if key == KEY_WEEKLY_ALL || key == KEY_USD_7D || key.starts_with(WEEKLY_MODEL_PREFIX) {
        Some(Duration::days(7))
    } else {
        None
    }
}

/// Pace of a window from a single reading (plus an optional earlier one).
///
/// The rate behind `projected_wall_at` is the slope between `previous` and now
/// when the previous sample is older, in the same window (no reset between —
/// utilization did not drop) and shows any growth; otherwise the window
/// average (`utilization / elapsed`). Returns `None` when the window has no
/// reset time or no known length.
pub fn compute(
    now: DateTime<Utc>,
    utilization: f64,
    resets_at: Option<DateTime<Utc>>,
    duration: Option<Duration>,
    previous: Option<Sample>,
) -> Option<Pace> {
    let resets_at = resets_at?;
    let duration = duration?;
    let total_secs = duration.num_seconds() as f64;
    if total_secs <= 0.0 {
        return None;
    }
    let remaining_secs = (resets_at - now).num_seconds().max(0) as f64;
    let elapsed_secs = (total_secs - remaining_secs).clamp(0.0, total_secs);
    let elapsed_fraction = elapsed_secs / total_secs;
    let expected_pct = elapsed_fraction * 100.0;
    let ratio = utilization.max(0.0) / expected_pct.max(MIN_EXPECTED_PCT);

    let rate_per_sec = previous
        .filter(|p| p.at < now && p.utilization <= utilization && p.utilization >= 0.0)
        .and_then(|p| {
            let dt = (now - p.at).num_seconds() as f64;
            let rate = (utilization - p.utilization) / dt;
            (rate > 0.0).then_some(rate)
        })
        .or_else(|| (elapsed_secs > 0.0 && utilization > 0.0).then(|| utilization / elapsed_secs));

    let projected_wall_at = if utilization >= 100.0 {
        Some(now)
    } else {
        rate_per_sec.and_then(|rate| {
            let secs = ((100.0 - utilization) / rate).ceil();
            (secs.is_finite() && secs < i64::MAX as f64)
                .then(|| now + Duration::seconds(secs as i64))
        })
    };

    Some(Pace { elapsed_fraction, expected_pct, ratio, projected_wall_at })
}

/// [`compute`] for a normalized window, keyed off its canonical key.
pub fn for_window(
    now: DateTime<Utc>,
    window: &crate::soft_limit::UsageWindow,
    previous: Option<Sample>,
) -> Option<Pace> {
    compute(now, window.utilization, window.resets_at, window_duration(&window.key), previous)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn durations_by_key() {
        assert_eq!(window_duration("session"), Some(Duration::hours(5)));
        assert_eq!(window_duration("usd_5h"), Some(Duration::hours(5)));
        assert_eq!(window_duration("weekly_all"), Some(Duration::days(7)));
        assert_eq!(window_duration("weekly_model:fable"), Some(Duration::days(7)));
        assert_eq!(window_duration("usd_7d"), Some(Duration::days(7)));
        assert_eq!(window_duration("session_usd"), None);
        assert_eq!(window_duration("nope"), None);
    }

    #[test]
    fn halfway_on_pace() {
        let now = t("2026-01-01T02:30:00Z");
        let resets = t("2026-01-01T05:00:00Z");
        let p = compute(now, 50.0, Some(resets), Some(Duration::hours(5)), None).unwrap();
        assert!((p.elapsed_fraction - 0.5).abs() < 1e-9);
        assert!((p.expected_pct - 50.0).abs() < 1e-9);
        assert!((p.ratio - 1.0).abs() < 1e-9);
        assert_eq!(p.projected_wall_at, Some(resets));
    }

    #[test]
    fn burst_over_pace_projects_wall_before_reset() {
        let now = t("2026-01-01T01:00:00Z");
        let resets = t("2026-01-01T05:00:00Z");
        let p = compute(now, 60.0, Some(resets), Some(Duration::hours(5)), None).unwrap();
        assert!((p.ratio - 3.0).abs() < 1e-9);
        let wall = p.projected_wall_at.unwrap();
        assert!(wall < resets);
        assert_eq!(wall, t("2026-01-01T01:40:00Z"));
    }

    #[test]
    fn idle_account_has_no_wall() {
        let now = t("2026-01-01T04:00:00Z");
        let resets = t("2026-01-01T05:00:00Z");
        let p = compute(now, 0.0, Some(resets), Some(Duration::hours(5)), None).unwrap();
        assert!(p.ratio.abs() < 1e-9);
        assert_eq!(p.projected_wall_at, None);
    }

    #[test]
    fn previous_sample_drives_the_rate() {
        let now = t("2026-01-01T02:00:00Z");
        let resets = t("2026-01-01T05:00:00Z");
        let prev = Sample { at: t("2026-01-01T01:50:00Z"), utilization: 40.0 };
        let p = compute(now, 50.0, Some(resets), Some(Duration::hours(5)), Some(prev)).unwrap();
        assert_eq!(p.projected_wall_at, Some(t("2026-01-01T02:50:00Z")));
    }

    #[test]
    fn previous_sample_after_a_reset_falls_back_to_window_average() {
        let now = t("2026-01-01T02:00:00Z");
        let resets = t("2026-01-01T05:00:00Z");
        let prev = Sample { at: t("2026-01-01T01:50:00Z"), utilization: 90.0 };
        let p = compute(now, 20.0, Some(resets), Some(Duration::hours(5)), Some(prev)).unwrap();
        assert_eq!(p.projected_wall_at, Some(t("2026-01-01T10:00:00Z")));
    }

    #[test]
    fn fresh_window_ratio_is_bounded() {
        let now = t("2026-01-01T00:00:00Z");
        let resets = t("2026-01-01T05:00:00Z");
        let p = compute(now, 5.0, Some(resets), Some(Duration::hours(5)), None).unwrap();
        assert!(p.elapsed_fraction.abs() < 1e-9);
        assert!((p.ratio - 5.0).abs() < 1e-9);
        assert_eq!(p.projected_wall_at, None);
    }

    #[test]
    fn saturated_window_walls_now() {
        let now = t("2026-01-01T01:00:00Z");
        let resets = t("2026-01-01T05:00:00Z");
        let p = compute(now, 100.0, Some(resets), Some(Duration::hours(5)), None).unwrap();
        assert_eq!(p.projected_wall_at, Some(now));
    }

    #[test]
    fn missing_reset_or_length_yields_nothing() {
        let now = t("2026-01-01T01:00:00Z");
        assert!(compute(now, 10.0, None, Some(Duration::hours(5)), None).is_none());
        assert!(compute(now, 10.0, Some(now), None, None).is_none());
    }
}
