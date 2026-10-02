//! Cycle a harness process the installed CLI has moved past. Harnesses supply
//! the versions and the busy evidence; the decision lives here.
//!
//! Busy counts can deadlock (a stale process breaks its work, which parks and
//! keeps counting), so a deferral with no busy noted for [`ESCALATE_AFTER`]
//! escalates to a cycle unless vetoed.

use std::time::{Duration, Instant};

/// Each check shells the harness binary; upgrades land a few times a day.
pub const CHECK_MIN_INTERVAL: Duration = Duration::from_mins(5);

pub const ESCALATE_AFTER: Duration = Duration::from_mins(30);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Nothing,
    Deferred { running: String, local: String },
    Cycle { running: String, local: String, escalated: bool },
}

/// `Some(true)` work is in flight, `Some(false)` idle, `None` unknown.
pub type Busy = Option<bool>;

/// First whitespace token that starts with a digit: `2.1.218 (Claude Code)`,
/// `codex-cli 0.153.4`, `1.18.7`.
#[must_use]
pub fn parse_cli_version(stdout: &str) -> Option<String> {
    stdout
        .split_whitespace()
        .find(|t| t.starts_with(|c: char| c.is_ascii_digit()))
        .map(str::to_owned)
}

/// A missing version decides nothing; unknown busy defers like busy does.
#[must_use]
pub fn decide(running: Option<&str>, local: Option<&str>, busy: Busy) -> Decision {
    let (Some(running), Some(local)) = (running, local) else {
        return Decision::Nothing;
    };
    if running == local {
        return Decision::Nothing;
    }
    let (running, local) = (running.to_owned(), local.to_owned());
    if busy == Some(false) {
        Decision::Cycle { running, local, escalated: false }
    } else {
        Decision::Deferred { running, local }
    }
}

#[derive(Debug, Default)]
pub struct VersionGate {
    last_check: Option<Instant>,
    /// Deferred pair and when work was last noted busy (or the deferral
    /// began), whichever is later.
    deferred: Option<(String, String, Instant)>,
    warned: Option<(String, String)>,
    vetoed: Option<(String, String)>,
}

impl VersionGate {
    /// Whether [`CHECK_MIN_INTERVAL`] has elapsed; records `now` when it has.
    pub fn due(&mut self, now: Instant) -> bool {
        let permit = self.last_check.is_none_or(|t| now.duration_since(t) >= CHECK_MIN_INTERVAL);
        if permit {
            self.last_check = Some(now);
        }
        permit
    }

    /// Reset the escalation clock: escalation requires quiescence across the
    /// whole window, not just at check time.
    pub fn note_busy(&mut self, now: Instant) {
        if let Some((_, _, since)) = self.deferred.as_mut() {
            *since = now;
        }
    }

    #[must_use]
    pub fn check(
        &mut self,
        running: Option<&str>,
        local: Option<&str>,
        busy: Busy,
        veto: bool,
        now: Instant,
    ) -> Decision {
        self.escalate(decide(running, local, busy), now, veto)
    }

    /// Upgrade a deferral of the same pair that sat [`ESCALATE_AFTER`] to a
    /// cycle; any other decision clears the clock. `veto` holds the deferral
    /// but leaves the clock armed, so the cycle happens once it lifts.
    #[must_use]
    pub fn escalate(&mut self, decision: Decision, now: Instant, veto: bool) -> Decision {
        let Decision::Deferred { running, local } = decision else {
            self.deferred = None;
            return decision;
        };
        let due = match &self.deferred {
            Some((r, l, since)) if *r == running && *l == local => {
                now.duration_since(*since) >= ESCALATE_AFTER
            }
            _ => {
                self.deferred = Some((running.clone(), local.clone(), now));
                false
            }
        };
        if !due {
            return Decision::Deferred { running, local };
        }
        if veto {
            if first_for(&mut self.vetoed, &running, &local) {
                tracing::warn!(
                    %running,
                    %local,
                    "version mismatch is past the escalation window but cycling is vetoed; \
                     deferring until the veto lifts"
                );
            }
            return Decision::Deferred { running, local };
        }
        self.deferred = None;
        Decision::Cycle { running, local, escalated: true }
    }

    /// True the first time this exact mismatch is seen, so a deferral logs
    /// once instead of every check.
    pub fn first_warning_for(&mut self, running: &str, local: &str) -> bool {
        first_for(&mut self.warned, running, local)
    }
}

fn first_for(seen: &mut Option<(String, String)>, running: &str, local: &str) -> bool {
    let pair = (running.to_owned(), local.to_owned());
    if seen.as_ref() == Some(&pair) {
        return false;
    }
    *seen = Some(pair);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deferred(r: &str, l: &str) -> Decision {
        Decision::Deferred { running: r.into(), local: l.into() }
    }

    fn cycle(r: &str, l: &str, escalated: bool) -> Decision {
        Decision::Cycle { running: r.into(), local: l.into(), escalated }
    }

    #[test]
    fn parses_the_first_numeric_token() {
        assert_eq!(parse_cli_version("1.18.7\n").as_deref(), Some("1.18.7"));
        assert_eq!(parse_cli_version(""), None);
        assert_eq!(parse_cli_version("some unexpected banner"), None);
    }

    #[test]
    fn decision_table() {
        assert_eq!(decide(Some("1"), Some("1"), Some(false)), Decision::Nothing);
        assert_eq!(decide(Some("1"), Some("1"), Some(true)), Decision::Nothing);
        assert_eq!(decide(None, Some("2"), Some(false)), Decision::Nothing);
        assert_eq!(decide(Some("1"), None, Some(false)), Decision::Nothing);
        assert_eq!(decide(Some("1"), Some("2"), Some(false)), cycle("1", "2", false));
        assert_eq!(decide(Some("1"), Some("2"), Some(true)), deferred("1", "2"));
        assert_eq!(decide(Some("1"), Some("2"), None), deferred("1", "2"));
    }

    #[test]
    fn due_permits_first_then_backs_off() {
        let mut g = VersionGate::default();
        let t0 = Instant::now();
        assert!(g.due(t0));
        assert!(!g.due(t0 + Duration::from_secs(1)));
        assert!(g.due(t0 + CHECK_MIN_INTERVAL));
    }

    #[test]
    fn a_deferral_escalates_after_the_quiet_window() {
        let mut g = VersionGate::default();
        let t0 = Instant::now();
        assert_eq!(g.escalate(deferred("1", "2"), t0, false), deferred("1", "2"));
        assert_eq!(
            g.escalate(deferred("1", "2"), t0 + ESCALATE_AFTER, false),
            cycle("1", "2", true)
        );
    }

    #[test]
    fn noted_busy_resets_the_clock() {
        let mut g = VersionGate::default();
        let t0 = Instant::now();
        g.note_busy(t0);
        let _ = g.escalate(deferred("1", "2"), t0, false);
        g.note_busy(t0 + ESCALATE_AFTER);
        assert_eq!(g.escalate(deferred("1", "2"), t0 + ESCALATE_AFTER, false), deferred("1", "2"));
        assert_eq!(
            g.escalate(deferred("1", "2"), t0 + ESCALATE_AFTER * 2, false),
            cycle("1", "2", true)
        );
    }

    #[test]
    fn a_new_pair_or_a_resolution_restarts_the_clock() {
        let mut g = VersionGate::default();
        let t0 = Instant::now();
        let _ = g.escalate(deferred("1", "2"), t0, false);
        assert_eq!(g.escalate(deferred("1", "3"), t0 + ESCALATE_AFTER, false), deferred("1", "3"));
        let _ = g.escalate(Decision::Nothing, t0 + ESCALATE_AFTER, false);
        assert_eq!(
            g.escalate(deferred("1", "3"), t0 + ESCALATE_AFTER * 2, false),
            deferred("1", "3")
        );
    }

    #[test]
    fn a_veto_holds_the_cycle_but_keeps_the_clock_armed() {
        let mut g = VersionGate::default();
        let t0 = Instant::now();
        let _ = g.escalate(deferred("1", "2"), t0, true);
        assert_eq!(g.escalate(deferred("1", "2"), t0 + ESCALATE_AFTER, true), deferred("1", "2"));
        assert_eq!(
            g.escalate(deferred("1", "2"), t0 + ESCALATE_AFTER, false),
            cycle("1", "2", true)
        );
    }

    #[test]
    fn warns_once_per_version_pair() {
        let mut g = VersionGate::default();
        assert!(g.first_warning_for("1", "2"));
        assert!(!g.first_warning_for("1", "2"));
        assert!(g.first_warning_for("1", "3"));
    }
}
