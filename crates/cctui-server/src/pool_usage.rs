//! Aggregating the quota windows of a pool's members into one gauge per
//! window: a level, a pace, and — when the history allows it — a projection of
//! when the pool as a whole runs dry.
//!
//! Three numbers, three different questions:
//!
//!   * **level** — the weighted mean utilization, each member counting for
//!     its `pool_weight` (the relative size of its plan, which upstream never
//!     reports). "The pool is 43% into its week."
//!   * **ratio** — weighted utilization over weighted linear budget, the same
//!     flame threshold the per-account cards use. "The pool burns 1.4× an even
//!     spend."
//!   * **projection** — a simulation of the pool's remaining capacity under
//!     the members' *measured* rates (two-point slopes over real samples, see
//!     [`crate::store::usage_samples`]), with the demand always served by the
//!     member holding the most headroom (what a `headroom` pool does at every
//!     launch and, with failover, mid-run) and each member refilling at its own
//!     reset. "All members hit 100% together in 2.2 days." A window average
//!     would extrapolate a fresh account's first burst into a regime, so the
//!     projection is withheld until every member has a real slope.
//!
//! The max of the members — what the header strip shows — is deliberately not
//! offered: it names the worst account, which is exactly what a pool exists to
//! route around.
//!
//! Pure: the route does the DB reads and the usage fetches.

use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;

use crate::pace::{self, Sample};

/// Simulation resolution. Ten minutes is coarse enough to keep a week under a
/// thousand steps and fine enough that a wall lands within the cache TTL.
const STEP: Duration = Duration::minutes(10);
/// How far the simulation looks: one weekly window, the longest there is.
const HORIZON: Duration = Duration::days(7);

/// Why a window carries no projection.
pub const UNAVAILABLE_INSUFFICIENT_HISTORY: &str = "insufficient_history";
pub const UNAVAILABLE_NO_RESET_TIME: &str = "no_reset_time";

/// One member's reading of one window, as the aggregate sees it.
#[derive(Debug, Clone)]
pub struct MemberWindow {
    pub account_id: Uuid,
    /// `accounts.pool_weight`: relative plan size, 1 when unknown.
    pub weight: f64,
    pub utilization: f64,
    pub resets_at: Option<DateTime<Utc>>,
    /// Canonical window length, `None` for a window that never resets.
    pub duration: Option<Duration>,
    /// An earlier reading of the same window instance, old enough for a slope
    /// (see [`crate::store::usage_samples::SLOPE_MIN_AGE`]); `None` when the
    /// history is too short.
    pub previous: Option<Sample>,
}

/// A member's share of one aggregated window.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct PoolUsageWindowMember {
    #[ts(type = "string")]
    pub account_id: Uuid,
    pub utilization: f64,
    #[ts(type = "string | null")]
    pub resets_at: Option<DateTime<Utc>>,
    pub expected_pct: Option<f64>,
    pub ratio: Option<f64>,
}

/// When the pool runs out under measured rates.
#[derive(Debug, Clone, PartialEq, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct PoolProjection {
    /// Every member at 100% at once; `None` when that never happens inside the
    /// horizon (a reset always comes first).
    #[ts(type = "string | null")]
    pub wall_at: Option<DateTime<Utc>>,
    /// The first member to hit 100%, which matters when failover is off.
    #[ts(type = "string | null")]
    pub first_member_wall_at: Option<DateTime<Utc>>,
    /// Sum of the members' measured rates, in percent points per hour.
    pub demand_pct_per_hour: f64,
    /// Shortest slope base among the members, in hours.
    pub slope_hours: f64,
    /// Lowest weighted mean headroom reached inside the horizon.
    pub min_margin_pct: f64,
}

/// One window of a pool family, aggregated.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct PoolUsageWindow {
    pub key: String,
    pub kind: String,
    pub label: String,
    pub model_display_name: Option<String>,
    /// Weighted mean utilization.
    pub level_pct: f64,
    /// Weighted mean of what an even spend would show now.
    pub expected_pct: f64,
    /// `level / expected`, the pool's burn against its linear budget; `None`
    /// on a window too young to rate.
    pub ratio: Option<f64>,
    /// Nearest reset among the members.
    #[ts(type = "string | null")]
    pub next_reset_at: Option<DateTime<Utc>>,
    pub members: Vec<PoolUsageWindowMember>,
    pub projection: Option<PoolProjection>,
    /// Set exactly when `projection` is `None`.
    pub projection_unavailable: Option<String>,
}

/// Identity of a window, shared by every member reading it.
#[derive(Debug, Clone)]
pub struct WindowIdentity {
    pub key: String,
    pub kind: String,
    pub label: String,
    pub model_display_name: Option<String>,
}

/// Aggregate one window across the members that report it.
pub fn aggregate_window(
    id: WindowIdentity,
    members: &[MemberWindow],
    now: DateTime<Utc>,
) -> PoolUsageWindow {
    let total_weight: f64 = members.iter().map(|m| m.weight).sum();
    let mut views = Vec::with_capacity(members.len());
    let mut weighted_util = 0.0;
    let mut weighted_expected = 0.0;
    let mut expected_weight = 0.0;
    for m in members {
        let p = pace::compute(now, m.utilization, m.resets_at, m.duration, None);
        weighted_util = m.weight.mul_add(m.utilization, weighted_util);
        if let Some(p) = &p {
            weighted_expected = m.weight.mul_add(p.expected_pct, weighted_expected);
            expected_weight += m.weight;
        }
        views.push(PoolUsageWindowMember {
            account_id: m.account_id,
            utilization: m.utilization,
            resets_at: m.resets_at,
            expected_pct: p.as_ref().map(|p| p.expected_pct),
            ratio: p.as_ref().map(|p| p.ratio),
        });
    }
    let level_pct = if total_weight > 0.0 { weighted_util / total_weight } else { 0.0 };
    let expected_pct =
        if expected_weight > 0.0 { weighted_expected / expected_weight } else { 0.0 };
    // Rate the pool's spend against its linear budget over the same members
    // the budget is known for: a member with no reset has no budget to rate.
    let ratio = (weighted_expected >= 1.0).then(|| {
        let util_of_rated: f64 = members
            .iter()
            .filter(|m| m.resets_at.is_some() && m.duration.is_some())
            .map(|m| m.weight * m.utilization)
            .sum();
        util_of_rated / weighted_expected
    });
    let next_reset_at = members.iter().filter_map(|m| m.resets_at).min();
    let (projection, projection_unavailable) = match project(members, now) {
        Ok(p) => (Some(p), None),
        Err(reason) => (None, Some(reason.to_owned())),
    };
    PoolUsageWindow {
        key: id.key,
        kind: id.kind,
        label: id.label,
        model_display_name: id.model_display_name,
        level_pct,
        expected_pct,
        ratio,
        next_reset_at,
        members: views,
        projection,
        projection_unavailable,
    }
}

/// A member's measured rate: percent points per hour between its previous
/// sample and now. `Err` names why it cannot be measured.
fn measured_rate(m: &MemberWindow, now: DateTime<Utc>) -> Result<(f64, f64), &'static str> {
    let Some(prev) = m.previous else {
        return Err(UNAVAILABLE_INSUFFICIENT_HISTORY);
    };
    let hours = (now - prev.at).num_seconds() as f64 / 3600.0;
    if hours <= 0.0 {
        return Err(UNAVAILABLE_INSUFFICIENT_HISTORY);
    }
    // A drop inside the same window instance is upstream noise (or a reset the
    // sampler did not see); it cannot be a negative demand.
    let rate = ((m.utilization - prev.utilization) / hours).max(0.0);
    Ok((rate, hours))
}

/// One member's bucket inside the simulation, in weighted points.
struct Slot {
    remaining: f64,
    /// 100 × weight.
    capacity: f64,
    next_reset: DateTime<Utc>,
    duration: Duration,
}

/// Simulate the pool's capacity under measured rates. Capacity and demand are
/// in *weighted* points (one member point × its weight) so a heavier plan's
/// percent counts for more; with unit weights this is plain percent points.
fn project(members: &[MemberWindow], now: DateTime<Utc>) -> Result<PoolProjection, &'static str> {
    if members.is_empty() {
        return Err(UNAVAILABLE_INSUFFICIENT_HISTORY);
    }
    let mut demand = 0.0; // weighted points per hour
    let mut demand_pct = 0.0; // plain points per hour, reported
    let mut slope_hours = f64::INFINITY;
    for m in members {
        if m.resets_at.is_none() || m.duration.is_none() {
            return Err(UNAVAILABLE_NO_RESET_TIME);
        }
        let (rate, hours) = measured_rate(m, now)?;
        demand = rate.mul_add(m.weight, demand);
        demand_pct += rate;
        slope_hours = slope_hours.min(hours);
    }
    let total_weight: f64 = members.iter().map(|m| m.weight).sum();

    let mut slots: Vec<Slot> = members
        .iter()
        .map(|m| Slot {
            remaining: ((100.0 - m.utilization).max(0.0)) * m.weight,
            capacity: 100.0 * m.weight,
            next_reset: m.resets_at.unwrap_or(now),
            duration: m.duration.unwrap_or(HORIZON),
        })
        .collect();

    let margin = |slots: &[Slot]| slots.iter().map(|s| s.remaining).sum::<f64>() / total_weight;
    let mut min_margin = margin(&slots);
    let mut first_member_wall_at = slots.iter().any(|s| s.remaining <= 0.0).then_some(now);
    let mut wall_at = None;
    let step_hours = STEP.num_seconds() as f64 / 3600.0;
    let mut t = Duration::zero();
    while t < HORIZON && wall_at.is_none() {
        t += STEP;
        let at = now + t;
        for s in &mut slots {
            while at >= s.next_reset {
                s.remaining = s.capacity;
                s.next_reset += s.duration;
            }
        }
        let mut need = demand * step_hours;
        // Serve from the member with the most headroom first, as a headroom
        // pool elects and as failover moves work.
        let mut order: Vec<usize> = (0..slots.len()).collect();
        order.sort_by(|a, b| slots[*b].remaining.total_cmp(&slots[*a].remaining));
        for i in order {
            if need <= 0.0 {
                break;
            }
            let take = need.min(slots[i].remaining);
            slots[i].remaining -= take;
            need -= take;
            if slots[i].remaining <= 0.0 && first_member_wall_at.is_none() {
                first_member_wall_at = Some(at);
            }
        }
        if need > 0.0 {
            wall_at = Some(at);
        }
        min_margin = min_margin.min(margin(&slots));
    }
    Ok(PoolProjection {
        wall_at,
        first_member_wall_at,
        demand_pct_per_hour: demand_pct,
        slope_hours: if slope_hours.is_finite() { slope_hours } else { 0.0 },
        min_margin_pct: min_margin,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    const NOW: &str = "2026-09-11T12:00:00Z";

    /// A member at `util`% whose window resets in `reset_h` hours, sampled
    /// `base_h` hours ago at `prev_util`.
    fn member(util: f64, reset_h: i64, previous: Option<(f64, i64)>, weight: f64) -> MemberWindow {
        let now = t(NOW);
        MemberWindow {
            account_id: Uuid::new_v4(),
            weight,
            utilization: util,
            resets_at: Some(now + Duration::hours(reset_h)),
            duration: Some(Duration::days(7)),
            previous: previous
                .map(|(u, h)| Sample { at: now - Duration::hours(h), utilization: u }),
        }
    }

    fn id() -> WindowIdentity {
        WindowIdentity {
            key: "weekly_all".into(),
            kind: "weekly_all".into(),
            label: "7d".into(),
            model_display_name: None,
        }
    }

    /// The study's synthetic case: two accounts at 50%, resets at 24h and
    /// 144h, demand 2.43 pt/h. Without the reset the pool would last
    /// 100 / 2.43 ≈ 41h; A refills at 24h, so the wall lands near 74h.
    #[test]
    fn simulation_refills_at_reset() {
        // A: 50% over 144h elapsed → 0.347 pt/h ; B: 50% over 24h → 2.083 pt/h.
        // Encode those as slopes over a 10h base.
        let a = member(50.0, 24, Some((50.0 - 3.47, 10)), 1.0);
        let b = member(50.0, 144, Some((50.0 - 20.83, 10)), 1.0);
        let w = aggregate_window(id(), &[a, b], t(NOW));
        assert!((w.level_pct - 50.0).abs() < 1e-9);
        let p = w.projection.expect("projection");
        assert!((p.demand_pct_per_hour - 2.43).abs() < 0.01, "{p:?}");
        let wall_h = (p.wall_at.unwrap() - t(NOW)).num_minutes() as f64 / 60.0;
        assert!((70.0..78.0).contains(&wall_h), "wall at {wall_h}h");
        assert!(p.first_member_wall_at.is_some());
        assert!((p.slope_hours - 10.0).abs() < 1e-9);
    }

    #[test]
    fn idle_pool_never_walls() {
        let a = member(50.0, 24, Some((50.0, 10)), 1.0);
        let b = member(50.0, 144, Some((50.0, 10)), 1.0);
        let w = aggregate_window(id(), &[a, b], t(NOW));
        let p = w.projection.expect("projection");
        assert_eq!(p.wall_at, None);
        assert_eq!(p.first_member_wall_at, None);
        assert!(p.demand_pct_per_hour.abs() < 1e-9);
        assert!((p.min_margin_pct - 50.0).abs() < 1e-9);
    }

    #[test]
    fn missing_history_on_one_member_withholds_the_projection() {
        let a = member(50.0, 24, Some((40.0, 10)), 1.0);
        let b = member(50.0, 144, None, 1.0);
        let w = aggregate_window(id(), &[a, b], t(NOW));
        assert!(w.projection.is_none());
        assert_eq!(w.projection_unavailable.as_deref(), Some(UNAVAILABLE_INSUFFICIENT_HISTORY));
        // Level and pace still come through.
        assert!((w.level_pct - 50.0).abs() < 1e-9);
        assert!(w.ratio.is_some());
    }

    #[test]
    fn weights_tilt_the_level() {
        // Two members at 20% and 80%: unit weights → 50%; the 80% account
        // four times as large → 68%.
        let a = member(20.0, 24, None, 1.0);
        let b = member(80.0, 24, None, 4.0);
        let w = aggregate_window(id(), &[a.clone(), b.clone()], t(NOW));
        assert!((w.level_pct - 68.0).abs() < 1e-9, "{}", w.level_pct);
        let mut a1 = a;
        a1.weight = 1.0;
        let mut b1 = b;
        b1.weight = 1.0;
        let w = aggregate_window(id(), &[a1, b1], t(NOW));
        assert!((w.level_pct - 50.0).abs() < 1e-9);
    }

    #[test]
    fn ratio_is_weighted_utilization_over_weighted_budget() {
        // Both members halfway through the week (84h elapsed of 168h): expected 50%.
        let a = member(60.0, 84, None, 1.0);
        let b = member(40.0, 84, None, 1.0);
        let w = aggregate_window(id(), &[a, b], t(NOW));
        assert!((w.expected_pct - 50.0).abs() < 1e-9);
        assert!((w.ratio.unwrap() - 1.0).abs() < 1e-9);
        assert_eq!(w.next_reset_at, Some(t(NOW) + Duration::hours(84)));
    }

    #[test]
    fn saturated_member_walls_immediately_but_pool_survives() {
        let a = member(100.0, 24, Some((99.0, 10)), 1.0);
        let b = member(10.0, 144, Some((9.0, 10)), 1.0);
        let w = aggregate_window(id(), &[a, b], t(NOW));
        let p = w.projection.expect("projection");
        assert_eq!(p.first_member_wall_at, Some(t(NOW)));
        // 0.2 pt/h against 90 points of room, refilled at 24h: no wall in a week.
        assert_eq!(p.wall_at, None);
    }

    #[test]
    fn no_reset_time_withholds_the_projection() {
        let mut a = member(50.0, 24, Some((40.0, 10)), 1.0);
        a.resets_at = None;
        let w = aggregate_window(id(), &[a], t(NOW));
        assert_eq!(w.projection_unavailable.as_deref(), Some(UNAVAILABLE_NO_RESET_TIME));
        assert!(w.ratio.is_none());
    }
}
