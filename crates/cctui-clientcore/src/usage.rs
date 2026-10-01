//! Usage-window readings: headroom tone, burn-rate state, reset countdowns and
//! dollar readouts.
//!
//! Ports the webui's `usage-battery.logic.ts` and the window half of
//! `cap-bar.logic.ts`, so a gauge in the TUI and a cap bar in the browser agree
//! on both the colour and the text. Times are unix milliseconds: the caller
//! supplies the clock.

/// Fill colour of a gauge, by headroom left in the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadroomTone {
    Ok,
    Warn,
    Danger,
    Unknown,
}

impl HeadroomTone {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Danger => "danger",
            Self::Unknown => "unknown",
        }
    }
}

/// Burn rate of a window: under the linear pace, on it, or past it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaceState {
    Leaf,
    Neutral,
    Flame,
}

impl PaceState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Leaf => "leaf",
            Self::Neutral => "neutral",
            Self::Flame => "flame",
        }
    }
}

const LEAF_BELOW: f64 = 0.8;
const FLAME_ABOVE: f64 = 1.2;

/// Green while more than half the window is left, amber down to 20%, then red.
#[must_use]
pub fn headroom_tone(utilization: Option<f64>) -> HeadroomTone {
    let Some(utilization) = utilization.filter(|u| u.is_finite()) else {
        return HeadroomTone::Unknown;
    };
    let headroom = 100.0 - utilization;
    if headroom > 50.0 {
        HeadroomTone::Ok
    } else if headroom > 20.0 {
        HeadroomTone::Warn
    } else {
        HeadroomTone::Danger
    }
}

/// `None` when the window is too young to have a rate.
#[must_use]
pub fn pace_state(ratio: Option<f64>) -> Option<PaceState> {
    let ratio = ratio.filter(|r| r.is_finite())?;
    Some(if ratio < LEAF_BELOW {
        PaceState::Leaf
    } else if ratio > FLAME_ABOVE {
        PaceState::Flame
    } else {
        PaceState::Neutral
    })
}

/// Utilization clamped to 0–100 and rounded, or `None` when not reported.
#[must_use]
pub fn bar_pct(utilization: Option<f64>) -> Option<i64> {
    let utilization = utilization.filter(|u| u.is_finite())?;
    Some(utilization.clamp(0.0, 100.0).round() as i64)
}

/// Compact countdown: `38 min`, `2h10`, `3d 4h`.
#[must_use]
pub fn countdown(ms: i64) -> String {
    let mins = (ms.max(0) as f64 / 60_000.0).round() as i64;
    if mins < 60 {
        return format!("{mins} min");
    }
    let hours = mins / 60;
    if hours < 24 {
        return format!("{hours}h{:02}", mins % 60);
    }
    format!("{}d {}h", hours / 24, hours % 24)
}

/// Full countdown to a reset, or `None` when it is unknown or already past.
#[must_use]
pub fn reset_in(resets_at_ms: Option<i64>, now_ms: i64) -> Option<String> {
    let at = resets_at_ms.filter(|at| *at > now_ms)?;
    Some(countdown(at - now_ms))
}

/// Largest unit only, for a readout column that holds `100% · resets 5d`.
#[must_use]
pub fn reset_in_short(resets_at_ms: Option<i64>, now_ms: i64) -> Option<String> {
    let at = resets_at_ms.filter(|at| *at > now_ms)?;
    let mins = (((at - now_ms) as f64 / 60_000.0).round() as i64).max(1);
    if mins < 60 {
        return Some(format!("{mins}m"));
    }
    let hours = mins / 60;
    if hours < 24 {
        return Some(format!("{hours}h"));
    }
    Some(format!("{}d", hours / 24))
}

/// Spend as a share of a dollar cap; `None` without a positive cap to measure
/// against, which is what makes an uncapped dollar window read as unknown.
#[must_use]
pub fn usd_pct(amount_usd: Option<f64>, cap_usd: Option<f64>) -> Option<i64> {
    let amount = amount_usd?;
    let cap = cap_usd.filter(|c| *c > 0.0)?;
    Some(((amount / cap) * 100.0).clamp(0.0, 100.0).round() as i64)
}

#[must_use]
pub fn money(n: f64) -> String {
    format!("${}", crate::format::js_to_fixed(n, 2))
}

/// `$12.40 / $60.00`, or just the spend when nothing caps it.
#[must_use]
pub fn usd_readout(amount_usd: Option<f64>, cap_usd: Option<f64>) -> Option<String> {
    let amount = amount_usd?;
    Some(cap_usd.map_or_else(|| money(amount), |cap| format!("{} / {}", money(amount), money(cap))))
}

/// Ms until the projected wall when it lands before the window resets — the
/// "you will hit the limit before it refills" case.
#[must_use]
pub fn wall_in_ms(wall_at_ms: Option<i64>, resets_at_ms: Option<i64>, now_ms: i64) -> Option<i64> {
    let (wall, reset) = (wall_at_ms?, resets_at_ms?);
    if wall >= reset {
        return None;
    }
    Some((wall - now_ms).max(0))
}

/// Index of the worst reading in `utilizations`: the highest reported one,
/// `None` when none of them reports a percentage.
#[must_use]
pub fn worst_index(utilizations: &[Option<f64>]) -> Option<usize> {
    utilizations
        .iter()
        .enumerate()
        .filter_map(|(i, u)| u.filter(|u| u.is_finite()).map(|u| (i, u)))
        .fold(None, |acc: Option<(usize, f64)>, (i, u)| match acc {
            Some((_, best)) if best >= u => acc,
            _ => Some((i, u)),
        })
        .map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::{
        HeadroomTone, PaceState, bar_pct, countdown, headroom_tone, money, pace_state, reset_in,
        reset_in_short, usd_pct, usd_readout, wall_in_ms, worst_index,
    };

    #[test]
    fn tones_follow_headroom_not_utilization() {
        assert_eq!(headroom_tone(None), HeadroomTone::Unknown);
        assert_eq!(headroom_tone(Some(f64::NAN)), HeadroomTone::Unknown);
        assert_eq!(headroom_tone(Some(0.0)), HeadroomTone::Ok);
        assert_eq!(headroom_tone(Some(50.0)), HeadroomTone::Warn);
        assert_eq!(headroom_tone(Some(79.9)), HeadroomTone::Warn);
        assert_eq!(headroom_tone(Some(80.0)), HeadroomTone::Danger);
        assert_eq!(headroom_tone(Some(120.0)), HeadroomTone::Danger);
    }

    #[test]
    fn a_rateless_window_has_no_pace_glyph() {
        assert_eq!(pace_state(None), None);
        assert_eq!(pace_state(Some(0.5)), Some(PaceState::Leaf));
        assert_eq!(pace_state(Some(1.0)), Some(PaceState::Neutral));
        assert_eq!(pace_state(Some(1.4)), Some(PaceState::Flame));
    }

    #[test]
    fn countdowns_read_as_the_largest_two_units() {
        assert_eq!(countdown(0), "0 min");
        assert_eq!(countdown(-5_000), "0 min");
        assert_eq!(countdown(38 * 60_000), "38 min");
        assert_eq!(countdown((2 * 60 + 10) * 60_000), "2h10");
        assert_eq!(countdown((2 * 60 + 5) * 60_000), "2h05");
        assert_eq!(countdown((3 * 24 + 4) * 3_600_000), "3d 4h");
    }

    #[test]
    fn a_reset_already_past_reads_as_nothing_rather_than_zero() {
        assert_eq!(reset_in(Some(100), 100), None);
        assert_eq!(reset_in(None, 0), None);
        assert_eq!(reset_in(Some(72 * 60_000), 0).as_deref(), Some("1h12"));
        assert_eq!(reset_in_short(Some(72 * 60_000), 0).as_deref(), Some("1h"));
        assert_eq!(reset_in_short(Some(30_000), 0).as_deref(), Some("1m"), "floored at a minute");
        assert_eq!(reset_in_short(Some(5 * 24 * 3_600_000), 0).as_deref(), Some("5d"));
    }

    #[test]
    fn a_dollar_window_without_a_cap_has_a_readout_but_no_share() {
        assert_eq!(usd_pct(Some(12.4), None), None);
        assert_eq!(usd_pct(Some(12.4), Some(0.0)), None);
        assert_eq!(usd_pct(None, Some(60.0)), None);
        assert_eq!(usd_pct(Some(12.4), Some(60.0)), Some(21));
        assert_eq!(usd_pct(Some(90.0), Some(60.0)), Some(100), "overage still reads full");
        assert_eq!(usd_readout(Some(12.4), None).as_deref(), Some("$12.40"));
        assert_eq!(usd_readout(Some(12.4), Some(60.0)).as_deref(), Some("$12.40 / $60.00"));
        assert_eq!(usd_readout(None, Some(60.0)), None);
        assert_eq!(money(0.0), "$0.00");
    }

    #[test]
    fn bar_percentages_clamp_rather_than_overflow_a_track() {
        assert_eq!(bar_pct(None), None);
        assert_eq!(bar_pct(Some(-4.0)), Some(0));
        assert_eq!(bar_pct(Some(78.4)), Some(78));
        assert_eq!(bar_pct(Some(104.0)), Some(100));
    }

    #[test]
    fn a_wall_after_the_reset_is_not_a_wall() {
        assert_eq!(wall_in_ms(Some(500), Some(400), 0), None);
        assert_eq!(wall_in_ms(Some(400), None, 0), None);
        assert_eq!(wall_in_ms(Some(400), Some(500), 0), Some(400));
        assert_eq!(wall_in_ms(Some(400), Some(500), 900), Some(0));
    }

    #[test]
    fn the_worst_window_is_the_fullest_one_that_reported() {
        assert_eq!(worst_index(&[]), None);
        assert_eq!(worst_index(&[None, None]), None);
        assert_eq!(worst_index(&[Some(31.0), None, Some(78.0)]), Some(2));
        assert_eq!(worst_index(&[Some(78.0), Some(78.0)]), Some(0), "ties keep the first");
    }
}
