//! Spend readings: what the windows cost, which models the dollars went to,
//! and the daily series a sparkline draws.
//!
//! Ports the webui's `spend.ts` and `overview/cache-loss.ts`. Per-model
//! dollars are attributed as the Overview's cost tile does: a session's
//! lifetime cost, booked to the window it was registered in. The daily series
//! is what `/sessions/stats/usage` reports per bucket: tokens and the dollars
//! they were priced at. Times are unix milliseconds — the caller's clock.

use crate::format;

/// One session's contribution to spend, pulled out of the list row.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionSpend {
    /// `None` when the harness never reported one; grouped as `unknown`.
    pub model: Option<String>,
    /// `None` for a session the server has no registration time for: it can be
    /// counted in the lifetime total but in no window.
    pub registered_at_ms: Option<i64>,
    pub cost_usd: f64,
    pub tokens: u64,
}

/// Window cut-offs, oldest first — anything registered at or after a cut-off
/// counts in that window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Windows {
    pub today_ms: i64,
    pub week_ms: i64,
    pub month_ms: i64,
}

/// A model's dollars in each window. `model` is the family, not the full id.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelSpendRow {
    pub model: String,
    pub today: f64,
    pub week: f64,
    pub month: f64,
}

/// Lifetime cost of the sessions registered at or after `cutoff_ms`.
///
/// This is what the session list can answer: not spend incurred inside the
/// window, but what the window's sessions have cost in total.
#[must_use]
pub fn spend_since(sessions: &[SessionSpend], cutoff_ms: i64) -> f64 {
    let total: f64 = sessions
        .iter()
        .filter(|s| s.registered_at_ms.is_some_and(|at| at >= cutoff_ms))
        .map(|s| s.cost_usd)
        .sum();
    // `Sum for f64` folds from -0.0, which formats as "-0.00".
    total + 0.0
}

/// Dollars per model family per window, biggest 30-day spender first.
///
/// Models are grouped by family so `claude-opus-5-20260101` and
/// `claude-opus-5[1m]` land on one row; a model-less session groups as
/// `unknown`. Rows that cost nothing in any window are dropped.
#[must_use]
pub fn model_spend(sessions: &[SessionSpend], windows: Windows) -> Vec<ModelSpendRow> {
    let mut rows: Vec<ModelSpendRow> = Vec::new();
    for session in sessions {
        let Some(at) = session.registered_at_ms else { continue };
        if at < windows.month_ms {
            continue;
        }
        let model = session
            .model
            .as_deref()
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .map_or_else(|| "unknown".to_owned(), format::model_family);
        if !rows.iter().any(|r| r.model == model) {
            rows.push(ModelSpendRow { model: model.clone(), today: 0.0, week: 0.0, month: 0.0 });
        }
        let Some(row) = rows.iter_mut().find(|r| r.model == model) else { continue };
        row.month += session.cost_usd;
        if at >= windows.week_ms {
            row.week += session.cost_usd;
        }
        if at >= windows.today_ms {
            row.today += session.cost_usd;
        }
    }
    rows.retain(|r| r.month > 0.0 || r.week > 0.0 || r.today > 0.0);
    rows.sort_by(|a, b| {
        b.month
            .partial_cmp(&a.month)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.model.cmp(&b.model))
    });
    rows
}

/// Column totals of a model table.
#[must_use]
pub fn spend_totals(rows: &[ModelSpendRow]) -> ModelSpendRow {
    ModelSpendRow {
        model: "total".to_owned(),
        today: rows.iter().map(|r| r.today).sum::<f64>() + 0.0,
        week: rows.iter().map(|r| r.week).sum::<f64>() + 0.0,
        month: rows.iter().map(|r| r.month).sum::<f64>() + 0.0,
    }
}

/// One day of the usage series, keyed by the local-midnight instant the caller
/// truncated it to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DailyPoint {
    pub day_ms: i64,
    pub tokens: u64,
    pub cost_usd: f64,
}

impl DailyPoint {
    /// One `/sessions/stats/usage` bucket, with its start already truncated to
    /// the local day. Tokens are what the bar stacks: input, output and cache
    /// reads, as the webui's `bucketTotal`.
    #[must_use]
    pub fn from_bucket(
        day_ms: i64,
        input: u64,
        output: u64,
        cache_read: u64,
        cost_usd: f64,
    ) -> Self {
        let cost_usd = if cost_usd.is_finite() { cost_usd.max(0.0) } else { 0.0 };
        Self { day_ms, tokens: input.saturating_add(output).saturating_add(cache_read), cost_usd }
    }
}

const DAY_MS: i64 = 86_400_000;

/// Zero-fill `points` into a dense oldest→newest series of `days` entries
/// ending at `end_day_ms`, so a sparkline has no gaps.
///
/// Mirrors the webui's `fillBuckets` at day granularity: both sides match rows
/// by their already-truncated local day, and an unmatched slot is a zero rather
/// than a missing bar.
#[must_use]
pub fn fill_daily(points: &[DailyPoint], days: usize, end_day_ms: i64) -> Vec<u64> {
    fill_with(points, days, end_day_ms, |p| p.tokens)
}

/// The dollar series of [`fill_daily`]: what the spend sparkline draws.
#[must_use]
pub fn fill_daily_usd(points: &[DailyPoint], days: usize, end_day_ms: i64) -> Vec<f64> {
    fill_with(points, days, end_day_ms, |p| p.cost_usd)
}

fn fill_with<T: Default + Copy>(
    points: &[DailyPoint],
    days: usize,
    end_day_ms: i64,
    pick: impl Fn(&DailyPoint) -> T,
) -> Vec<T> {
    (0..days)
        .rev()
        .map(|back| {
            let ms = end_day_ms - i64::try_from(back).unwrap_or(0) * DAY_MS;
            points.iter().find(|p| p.day_ms == ms).map_or_else(T::default, &pick)
        })
        .collect()
}

/// Dollars across the whole series, the caption under a spend sparkline.
#[must_use]
pub fn daily_total_usd(points: &[DailyPoint]) -> f64 {
    points.iter().map(|p| p.cost_usd).sum::<f64>() + 0.0
}

/// Range totals of the daily cache-loss rows: dollars burned re-sending a
/// prompt whose cache had gone, and how many busts did it.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CacheLossTotals {
    pub usd: f64,
    pub tokens: u64,
    pub busts: u64,
}

/// One day of cache loss, as `/sessions/stats/cache-busts` reports it.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct DailyCacheLoss {
    pub usd: f64,
    pub lost_tokens: u64,
    pub busts: u64,
}

/// Ports `cacheLossTotals`.
#[must_use]
pub fn cache_loss_totals(days: &[DailyCacheLoss]) -> CacheLossTotals {
    CacheLossTotals {
        usd: days.iter().map(|d| d.usd).sum::<f64>() + 0.0,
        tokens: days.iter().map(|d| d.lost_tokens).sum(),
        busts: days.iter().map(|d| d.busts).sum(),
    }
}

/// The per-session Langfuse rollup, as the cost line needs it.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LangfuseSpend {
    pub cost_usd: f64,
    pub trace_count: u64,
}

/// Whether a Langfuse rollup is worth a cost line at all.
///
/// A session with no traces yet is not: the webui's chip hides on the same
/// test, so a missing or empty response shows nothing on either side.
#[must_use]
pub const fn has_langfuse_cost(usage: Option<LangfuseSpend>) -> bool {
    match usage {
        Some(u) => u.trace_count > 0,
        None => false,
    }
}

/// The Langfuse cost as the chip writes it: cents under a dollar, mills over.
///
/// Deliberately not `format::usd` — the chip keeps a third digit on sub-dollar
/// sessions, which is where most of them sit, and the TUI must read the same.
#[must_use]
pub fn langfuse_cost_label(cost_usd: f64) -> String {
    let cost = if cost_usd.is_finite() { cost_usd } else { 0.0 };
    format!("${}", crate::format::js_to_fixed(cost, if cost >= 1.0 { 2 } else { 3 }))
}

#[cfg(test)]
mod tests {
    use super::{
        DailyPoint, SessionSpend, Windows, daily_total_usd, fill_daily, fill_daily_usd,
        langfuse_cost_label, model_spend, spend_since, spend_totals,
    };

    const DAY: i64 = 86_400_000;

    fn session(model: Option<&str>, at: Option<i64>, cost: f64) -> SessionSpend {
        SessionSpend {
            model: model.map(ToOwned::to_owned),
            registered_at_ms: at,
            cost_usd: cost,
            tokens: 0,
        }
    }

    #[test]
    fn spend_since_ignores_undated_and_older_sessions() {
        let rows =
            [session(None, Some(100), 1.0), session(None, Some(99), 2.0), session(None, None, 4.0)];
        assert!((spend_since(&rows, 100) - 1.0).abs() < 1e-9);
        assert!(spend_since(&[], 0).abs() < 1e-9);
    }

    #[test]
    fn model_spend_groups_by_family_and_ranks_by_month() {
        let windows = Windows { today_ms: 30 * DAY, week_ms: 24 * DAY, month_ms: 0 };
        let rows = model_spend(
            &[
                session(Some("claude-opus-5-20260101"), Some(30 * DAY), 4.0),
                session(Some("claude-opus-5[1m]"), Some(25 * DAY), 1.0),
                session(Some("claude-sonnet-5"), Some(DAY), 9.0),
                session(None, Some(30 * DAY), 0.5),
                session(Some("gpt-5.6"), Some(-DAY), 100.0),
            ],
            windows,
        );
        let names: Vec<&str> = rows.iter().map(|r| r.model.as_str()).collect();
        assert_eq!(names, ["sonnet", "opus", "unknown"]);
        let opus = &rows[1];
        assert!((opus.month - 5.0).abs() < 1e-9);
        assert!((opus.week - 5.0).abs() < 1e-9);
        assert!((opus.today - 4.0).abs() < 1e-9);
        let totals = spend_totals(&rows);
        assert!((totals.month - 14.5).abs() < 1e-9);
    }

    #[test]
    fn fill_daily_zero_fills_gaps() {
        let end = 10 * DAY;
        let points = [
            DailyPoint { day_ms: end, tokens: 7, cost_usd: 0.5 },
            DailyPoint { day_ms: end - 2 * DAY, tokens: 3, cost_usd: 1.25 },
        ];
        assert_eq!(fill_daily(&points, 4, end), vec![0, 3, 0, 7]);
        assert!(fill_daily(&points, 0, end).is_empty());
        assert_eq!(fill_daily_usd(&points, 4, end), vec![0.0, 1.25, 0.0, 0.5]);
        assert!((daily_total_usd(&points) - 1.75).abs() < 1e-9);
        assert!(daily_total_usd(&[]).abs() < 1e-9);
        assert!(!daily_total_usd(&[]).is_sign_negative());
    }

    #[test]
    fn daily_point_stacks_tokens_and_keeps_dollars_finite() {
        let p = DailyPoint::from_bucket(DAY, 100, 20, 5, 0.25);
        assert_eq!(p.tokens, 125);
        assert!((p.cost_usd - 0.25).abs() < 1e-9);
        assert_eq!(DailyPoint::from_bucket(DAY, 1, 1, 1, f64::NAN).cost_usd, 0.0);
        assert_eq!(DailyPoint::from_bucket(DAY, 1, 1, 1, -3.0).cost_usd, 0.0);
    }

    #[test]
    fn langfuse_label_keeps_mills_under_a_dollar() {
        assert_eq!(langfuse_cost_label(0.0234), "$0.023");
        assert_eq!(langfuse_cost_label(1.5), "$1.50");
        assert_eq!(langfuse_cost_label(f64::NAN), "$0.000");
    }
}
