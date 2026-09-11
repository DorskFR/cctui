//! The quota time series behind the real pace: one row per credential, per
//! window, per upstream fetch (see migration 106).
//!
//! Two operations, both deliberately narrow. [`record`] appends the windows of
//! a fresh fetch and prunes the credential's stale rows; it never fails the
//! caller, because losing a sample costs a slightly worse rate later, while
//! failing a usage read would blank a gauge now. [`previous_for`] finds the
//! reading a two-point slope should rate against: old enough for the integer
//! percentages upstream to have moved, and in the same window instance, so a
//! reset in between never reads as negative growth.

use chrono::{DateTime, Duration, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

use crate::pace::{Sample, window_duration};
use crate::soft_limit::UsageWindow;

/// Youngest a previous sample may be for a slope to mean anything: upstream
/// percentages are integers, and on a weekly window one point is ~1h40 of
/// even spend, so a shorter base reads as zero or as a one-point jump.
pub const SLOPE_MIN_AGE: Duration = Duration::hours(2);

/// How long a credential's samples are kept: one weekly window plus slack.
const RETENTION_DAYS: i64 = 8;

/// A previous reading of one window, keyed so a caller can match its windows
/// in one pass.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PreviousSample {
    pub window_key: String,
    pub utilization: f64,
    pub resets_at: Option<DateTime<Utc>>,
    pub sampled_at: DateTime<Utc>,
}

impl PreviousSample {
    pub const fn sample(&self) -> Sample {
        Sample { at: self.sampled_at, utilization: self.utilization }
    }
}

/// Whether a window is worth sampling: dollar windows have no percentage to
/// rate, and a window with no reset has no length to rate against.
fn samplable(w: &UsageWindow) -> bool {
    w.kind != "usd" && !w.key.starts_with("usd_") && w.resets_at.is_some()
}

/// Append one row per samplable window and prune the credential's rows older
/// than [`RETENTION_DAYS`]. Best-effort by contract: errors are logged, never
/// returned — the caller has a usage payload to serve and must not lose it to
/// bookkeeping.
pub async fn record(
    pool: &sqlx::PgPool,
    provider_id: Uuid,
    windows: &[UsageWindow],
    now: DateTime<Utc>,
) {
    for w in windows.iter().filter(|w| samplable(w)) {
        if let Err(e) = sqlx::query(
            "INSERT INTO account_usage_samples \
                 (provider_id, window_key, utilization, resets_at, sampled_at) \
             VALUES ($1, $2, $3, $4, $5) ON CONFLICT DO NOTHING",
        )
        .bind(provider_id)
        .bind(&w.key)
        .bind(w.utilization)
        .bind(w.resets_at)
        .bind(now)
        .execute(pool)
        .await
        {
            tracing::warn!(
                provider_id = %provider_id, key = %w.key, error = %e,
                "recording usage sample failed"
            );
            return;
        }
    }
    if let Err(e) =
        sqlx::query("DELETE FROM account_usage_samples WHERE provider_id = $1 AND sampled_at < $2")
            .bind(provider_id)
            .bind(now - Duration::days(RETENTION_DAYS))
            .execute(pool)
            .await
    {
        tracing::warn!(provider_id = %provider_id, error = %e, "pruning usage samples failed");
    }
}

/// For each of `windows`, the newest sample at least [`SLOPE_MIN_AGE`] old
/// that belongs to the same window instance (`resets_at` within a minute) and
/// is no older than the window's own length. One query per credential; a
/// window with no qualifying sample is simply absent from the result.
pub async fn previous_for(
    exec: impl PgExecutor<'_>,
    provider_id: Uuid,
    windows: &[UsageWindow],
    now: DateTime<Utc>,
) -> Result<Vec<PreviousSample>, sqlx::Error> {
    let cutoff = now - SLOPE_MIN_AGE;
    // No window is longer than a week, so this only trims the scan; the
    // per-window bound is applied in `matches_window`.
    let oldest = now - Duration::days(7);
    let rows: Vec<PreviousSample> = sqlx::query_as(
        "SELECT DISTINCT ON (window_key) window_key, utilization, resets_at, sampled_at \
           FROM account_usage_samples \
          WHERE provider_id = $1 AND sampled_at <= $2 AND sampled_at >= $3 \
          ORDER BY window_key, sampled_at DESC",
    )
    .bind(provider_id)
    .bind(cutoff)
    .bind(oldest)
    .fetch_all(exec)
    .await?;
    Ok(rows.into_iter().filter(|r| matches_window(r, windows, now)).collect())
}

/// Same window instance and within the window's length. Pure so the match
/// rule is testable without a database.
pub fn matches_window(row: &PreviousSample, windows: &[UsageWindow], now: DateTime<Utc>) -> bool {
    let Some(w) = windows.iter().find(|w| w.key == row.window_key) else {
        return false;
    };
    let Some(len) = window_duration(&w.key) else {
        return false;
    };
    if now - row.sampled_at > len {
        return false;
    }
    match (row.resets_at, w.resets_at) {
        (Some(a), Some(b)) => (a - b).num_seconds().abs() < 60,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn window(key: &str, resets: &str) -> UsageWindow {
        UsageWindow {
            key: key.into(),
            kind: "weekly_all".into(),
            label: "7d".into(),
            utilization: 50.0,
            amount_usd: None,
            resets_at: Some(t(resets)),
            model_id: None,
            model_display_name: None,
        }
    }

    fn row(key: &str, resets: &str, at: &str) -> PreviousSample {
        PreviousSample {
            window_key: key.into(),
            utilization: 40.0,
            resets_at: Some(t(resets)),
            sampled_at: t(at),
        }
    }

    #[test]
    fn same_window_instance_matches() {
        let now = t("2026-09-11T12:00:00Z");
        let w = [window("weekly_all", "2026-09-15T04:00:00.362Z")];
        let r = row("weekly_all", "2026-09-15T04:00:00Z", "2026-09-11T09:00:00Z");
        assert!(matches_window(&r, &w, now));
    }

    #[test]
    fn a_reset_in_between_does_not_match() {
        let now = t("2026-09-11T12:00:00Z");
        let w = [window("weekly_all", "2026-09-15T04:00:00Z")];
        let r = row("weekly_all", "2026-09-08T04:00:00Z", "2026-09-11T09:00:00Z");
        assert!(!matches_window(&r, &w, now));
    }

    #[test]
    fn older_than_the_window_does_not_match() {
        let now = t("2026-09-11T12:00:00Z");
        let w = [window("session", "2026-09-11T13:00:00Z")];
        let r = row("session", "2026-09-11T13:00:00Z", "2026-09-11T06:00:00Z");
        assert!(!matches_window(&r, &w, now));
    }

    #[test]
    fn unknown_key_does_not_match() {
        let now = t("2026-09-11T12:00:00Z");
        let w = [window("weekly_all", "2026-09-15T04:00:00Z")];
        let r = row("session", "2026-09-15T04:00:00Z", "2026-09-11T09:00:00Z");
        assert!(!matches_window(&r, &w, now));
    }

    #[test]
    fn dollar_and_resetless_windows_are_not_sampled() {
        let mut usd = window("usd_7d", "2026-09-15T04:00:00Z");
        usd.kind = "usd".into();
        assert!(!samplable(&usd));
        let mut no_reset = window("weekly_all", "2026-09-15T04:00:00Z");
        no_reset.resets_at = None;
        assert!(!samplable(&no_reset));
        assert!(samplable(&window("weekly_all", "2026-09-15T04:00:00Z")));
    }
}
