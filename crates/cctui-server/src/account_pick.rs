//! Ranking the candidate accounts an `auto` spawn may bind to.
//!
//! `POST /spawn` with `auto_account` asks the server to choose between the
//! caller's accounts instead of refusing to guess. "Best" here means *most
//! allocation left for the session about to run*, which is not the same as
//! "least used": an account can sit at 0% of its 5h window and still be at 100%
//! of its weekly one, so it would rate-limit on the first request. The score is
//! therefore the account's **narrowest** margin across the windows that apply,
//! and the winner is the account whose narrowest margin is widest.
//!
//! Three rules shape which windows apply:
//!
//!   * model-scoped windows count only for the model being launched — an
//!     account whose weekly Fable budget is spent is still fine for an Opus
//!     session;
//!   * the account's own configured soft limits count, so `auto` never elects
//!     an account the gateway would 429 moments later;
//!   * dollar windows are excluded from the margin (a percent of a USD budget
//!     means nothing), though their caps still block through the soft limit.
//!
//! This module is pure: the caller does the DB reads and the usage fetches, so
//! every rule above is unit-testable without a database or a network.

use chrono::{DateTime, Utc};

use crate::soft_limit::{
    Decision, SoftLimits, UsageWindow, WEEKLY_MODEL_PREFIX, evaluate_soft_limit, slug,
};

/// One account the spawn could bind to, as the ranker sees it.
#[derive(Debug, Clone)]
pub struct Candidate {
    /// Account name — what a spawn binds and what the error/toast shows.
    pub name: String,
    /// Normalized usage windows for the credential that will actually serve.
    /// Empty when usage could not be read; see `usage_known`.
    pub windows: Vec<UsageWindow>,
    /// The account's configured caps.
    pub limits: SoftLimits,
    /// Whether `windows` reflects a real reading. An account whose usage
    /// endpoint failed has no windows, which must NOT be read as "wide open"
    /// nor as "exhausted" — only as unknown.
    pub usage_known: bool,
}

/// Why one candidate is out of the running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blocked {
    pub name: String,
    /// Human-readable cause, naming the window and its reset.
    pub reason: String,
}

/// The outcome of ranking.
#[derive(Debug, Clone, PartialEq)]
pub enum Pick {
    /// Bind this account. `headroom_pct` is the winning margin, `None` when the
    /// pick was made without usage data.
    Chosen { name: String, headroom_pct: Option<f64> },
    /// Every candidate was readable AND out of allocation.
    Exhausted(Vec<Blocked>),
    /// No candidate at all.
    None,
}

/// Whether a normalized window applies to the model this spawn will run.
///
/// Non-scoped windows (5h, weekly-all) always apply. A scoped window applies
/// only when its model matches the requested one; with no model requested we
/// cannot tell, so every window applies (the conservative side: it can only
/// narrow the margin, never overstate it).
fn window_applies(window: &UsageWindow, model: Option<&str>) -> bool {
    let Some(scoped) = window.key.strip_prefix(WEEKLY_MODEL_PREFIX) else { return true };
    let Some(model) = model else { return true };
    let requested = slug(model);
    if requested.is_empty() || scoped.is_empty() {
        return true;
    }
    // Either direction: the request may be an alias the window spells out
    // (`fable` vs `claude-fable-5`) or a fuller id than the window's
    // (`claude-opus-4-8-1m` vs `claude-opus-4-8`).
    requested.contains(scoped) || scoped.contains(&requested)
}

/// Percent windows carry a meaningful margin; dollar ones do not.
const fn is_percent_window(window: &UsageWindow) -> bool {
    window.amount_usd.is_none()
}

/// The narrowest margin across the applicable percent windows, or `None` when
/// there are none to measure.
fn narrowest_margin(windows: &[&UsageWindow]) -> Option<f64> {
    windows
        .iter()
        .filter(|w| is_percent_window(w))
        .map(|w| 100.0 - w.utilization)
        .fold(None, |acc: Option<f64>, margin| Some(acc.map_or(margin, |a| a.min(margin))))
}

/// A window at or past 100% is spent regardless of any configured cap.
fn spent_window<'w>(windows: &[&'w UsageWindow]) -> Option<&'w UsageWindow> {
    windows.iter().find(|w| is_percent_window(w) && w.utilization >= 100.0).copied()
}

fn resets_phrase(window: &UsageWindow, now: DateTime<Utc>) -> String {
    let Some(resets_at) = window.resets_at else { return String::new() };
    let mins = (resets_at - now).num_minutes();
    if mins <= 0 {
        return String::new();
    }
    if mins < 60 {
        format!(", resets in {mins}m")
    } else {
        format!(", resets in {}h", (mins + 59) / 60)
    }
}

/// Rank `candidates` and pick one.
///
/// Ordering: accounts with a known, positive margin first (widest margin wins,
/// ties broken by name so the choice is deterministic); then accounts whose
/// usage could not be read. An unreadable account ranks behind every readable
/// one with room — we have positive evidence for the latter and none for the
/// former — but it stays eligible, so a flaky usage endpoint degrades the
/// choice instead of blocking the launch.
///
/// [`Pick::Exhausted`] is returned only when every candidate was read AND every
/// one is out. "Usage unavailable" must never masquerade as "you are out of
/// allocation".
pub fn pick_account(candidates: &[Candidate], model: Option<&str>, now: DateTime<Utc>) -> Pick {
    if candidates.is_empty() {
        return Pick::None;
    }

    let mut ranked: Vec<(f64, &str)> = Vec::new();
    let mut unknown: Vec<&str> = Vec::new();
    let mut blocked: Vec<Blocked> = Vec::new();

    for candidate in candidates {
        if !candidate.usage_known {
            unknown.push(&candidate.name);
            continue;
        }
        let applicable: Vec<&UsageWindow> =
            candidate.windows.iter().filter(|w| window_applies(w, model)).collect();

        // A spent window is disqualifying on its own; a configured cap
        // disqualifies through the same evaluator the gateway uses, so `auto`
        // and the gateway can never disagree about who is available.
        if let Some(window) = spent_window(&applicable) {
            blocked.push(Blocked {
                name: candidate.name.clone(),
                reason: format!(
                    "{} at {}%{}",
                    window.label,
                    window.utilization.round() as i64,
                    resets_phrase(window, now)
                ),
            });
            continue;
        }
        let owned: Vec<UsageWindow> = applicable.iter().map(|w| (*w).clone()).collect();
        if let Decision::Block { reason, .. } = evaluate_soft_limit(&owned, &candidate.limits, now)
        {
            blocked.push(Blocked { name: candidate.name.clone(), reason });
            continue;
        }

        match narrowest_margin(&applicable) {
            // Readable, but nothing measurable (no percent window at all):
            // eligible, yet with no margin to compare, so it queues with the
            // unknowns rather than outranking a measured account.
            None => unknown.push(&candidate.name),
            Some(margin) => ranked.push((margin, &candidate.name)),
        }
    }

    // Widest margin first; name ascending breaks ties so repeated spawns under
    // identical usage always land on the same account.
    ranked.sort_by(|a, b| {
        b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(b.1))
    });
    if let Some((margin, name)) = ranked.first() {
        return Pick::Chosen { name: (*name).to_owned(), headroom_pct: Some(*margin) };
    }
    unknown.sort_unstable();
    if let Some(name) = unknown.first() {
        return Pick::Chosen { name: (*name).to_owned(), headroom_pct: None };
    }
    blocked.sort_by(|a, b| a.name.cmp(&b.name));
    Pick::Exhausted(blocked)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::soft_limit::{KEY_SESSION, KEY_WEEKLY_ALL, SoftLimit};
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 31, 12, 0, 0).unwrap()
    }

    fn window(key: &str, label: &str, utilization: f64, resets_in_hours: i64) -> UsageWindow {
        UsageWindow {
            key: key.to_owned(),
            kind: "session".to_owned(),
            label: label.to_owned(),
            utilization,
            amount_usd: None,
            resets_at: Some(now() + chrono::Duration::hours(resets_in_hours)),
            model_id: None,
            model_display_name: None,
        }
    }

    fn candidate(name: &str, windows: Vec<UsageWindow>) -> Candidate {
        Candidate {
            name: name.to_owned(),
            windows,
            limits: SoftLimits::default(),
            usage_known: true,
        }
    }

    /// The accounts page as screenshotted when this was specified: Claudo idle
    /// on its 5h window but weekly-spent, Patrigeon busier on 5h but with room
    /// everywhere. Ranking on 5h alone would elect Claudo, which cannot serve a
    /// single request.
    fn real_world() -> Vec<Candidate> {
        vec![
            candidate(
                "Claudo",
                vec![
                    window(KEY_SESSION, "5h", 0.0, 1),
                    window(KEY_WEEKLY_ALL, "Weekly (all models)", 100.0, 16),
                    window("weekly_model:fable", "Weekly Fable", 85.0, 16),
                ],
            ),
            candidate(
                "Patrigeon",
                vec![
                    window(KEY_SESSION, "5h", 15.0, 3),
                    window(KEY_WEEKLY_ALL, "Weekly (all models)", 48.0, 96),
                    window("weekly_model:fable", "Weekly Fable", 4.0, 96),
                ],
            ),
        ]
    }

    #[test]
    fn picks_the_widest_narrowest_margin_not_the_idlest_window() {
        let pick = pick_account(&real_world(), Some("opus"), now());
        assert_eq!(pick, Pick::Chosen { name: "Patrigeon".to_owned(), headroom_pct: Some(52.0) });
    }

    #[test]
    fn a_scoped_window_only_counts_for_its_own_model() {
        // One account, plenty of general room, but its Fable budget is spent.
        let candidates = vec![candidate(
            "Solo",
            vec![
                window(KEY_SESSION, "5h", 5.0, 1),
                window(KEY_WEEKLY_ALL, "Weekly (all models)", 10.0, 16),
                window("weekly_model:claude-fable-5", "Weekly Fable", 100.0, 16),
            ],
        )];

        // An Opus session does not care about the Fable window.
        assert_eq!(
            pick_account(&candidates, Some("opus"), now()),
            Pick::Chosen { name: "Solo".to_owned(), headroom_pct: Some(90.0) }
        );

        // A Fable session does, and there is nothing left for it. The alias
        // `fable` must match the window's `claude-fable-5` key.
        let Pick::Exhausted(blocked) = pick_account(&candidates, Some("fable"), now()) else {
            panic!("expected exhaustion on the Fable window")
        };
        assert!(blocked[0].reason.contains("Weekly Fable at 100%"), "{:?}", blocked[0]);
    }

    #[test]
    fn an_unknown_model_keeps_every_window() {
        let Pick::Chosen { name, headroom_pct } = pick_account(&real_world(), None, now()) else {
            panic!("expected a pick")
        };
        assert_eq!(name, "Patrigeon");
        assert_eq!(headroom_pct, Some(52.0));
    }

    #[test]
    fn a_spent_window_disqualifies_even_with_no_cap_configured() {
        let only_claudo = vec![real_world().remove(0)];
        let Pick::Exhausted(blocked) = pick_account(&only_claudo, Some("opus"), now()) else {
            panic!("expected exhaustion")
        };
        assert_eq!(blocked.len(), 1);
        assert_eq!(blocked[0].name, "Claudo");
        assert!(blocked[0].reason.contains("Weekly (all models) at 100%"), "{:?}", blocked[0]);
        assert!(blocked[0].reason.contains("resets in 16h"), "{:?}", blocked[0]);
    }

    #[test]
    fn a_configured_cap_disqualifies_before_the_window_is_spent() {
        let mut candidates = real_world();
        candidates[1].limits.limits.insert(
            KEY_WEEKLY_ALL.to_owned(),
            SoftLimit { cap_pct: Some(40), cap_usd: None, bypass_minutes: None },
        );
        // Patrigeon is at 48% against a 40% cap, Claudo is weekly-spent: nobody left.
        let Pick::Exhausted(blocked) = pick_account(&candidates, Some("opus"), now()) else {
            panic!("expected exhaustion")
        };
        assert_eq!(blocked.len(), 2);
        assert_eq!(blocked[0].name, "Claudo");
        assert!(blocked[1].reason.contains("cap 40%"), "{:?}", blocked[1]);
    }

    #[test]
    fn unreadable_usage_ranks_behind_a_measured_account_but_stays_eligible() {
        let mut candidates = real_world();
        candidates.push(Candidate {
            name: "Alphabetically-first".to_owned(),
            windows: vec![],
            limits: SoftLimits::default(),
            usage_known: false,
        });
        // Patrigeon is measured and has room, so it wins despite the unknown
        // sorting first by name.
        let Pick::Chosen { name, .. } = pick_account(&candidates, Some("opus"), now()) else {
            panic!("expected a pick")
        };
        assert_eq!(name, "Patrigeon");
    }

    #[test]
    fn unreadable_usage_never_reads_as_exhausted() {
        let candidates = vec![
            Candidate {
                name: "Zeta".to_owned(),
                windows: vec![],
                limits: SoftLimits::default(),
                usage_known: false,
            },
            Candidate {
                name: "Alpha".to_owned(),
                windows: vec![],
                limits: SoftLimits::default(),
                usage_known: false,
            },
        ];
        // No data anywhere: deterministic fallback, never an error.
        assert_eq!(
            pick_account(&candidates, Some("opus"), now()),
            Pick::Chosen { name: "Alpha".to_owned(), headroom_pct: None }
        );
    }

    #[test]
    fn a_spent_account_alongside_an_unreadable_one_still_launches() {
        let mut candidates = vec![real_world().remove(0)];
        candidates.push(Candidate {
            name: "Unknown".to_owned(),
            windows: vec![],
            limits: SoftLimits::default(),
            usage_known: false,
        });
        assert_eq!(
            pick_account(&candidates, Some("opus"), now()),
            Pick::Chosen { name: "Unknown".to_owned(), headroom_pct: None }
        );
    }

    #[test]
    fn ties_break_on_name_so_the_choice_is_reproducible() {
        let candidates = vec![
            candidate("Zeta", vec![window(KEY_SESSION, "5h", 10.0, 1)]),
            candidate("Alpha", vec![window(KEY_SESSION, "5h", 10.0, 1)]),
        ];
        assert_eq!(
            pick_account(&candidates, Some("opus"), now()),
            Pick::Chosen { name: "Alpha".to_owned(), headroom_pct: Some(90.0) }
        );
    }

    #[test]
    fn no_candidates_is_not_exhaustion() {
        assert_eq!(pick_account(&[], Some("opus"), now()), Pick::None);
    }
}
