//! Proactive gateway-env healing for autonomously-respawned workers (CCT-462).
//!
//! Background (CCT-460): the claude adapter's launch chokepoint
//! ([`Driver::resolve_launch_env`](crate::adapters::claude_code)) re-derives a
//! session's gateway-routing env from the server's durable
//! `sessions.account_id` binding on every CCTUI-INITIATED launch
//! (spawn / resume / cold-resume / fork) and is fail-closed. That covers every
//! relaunch cctui drives.
//!
//! BUT the on-demand `claude daemon` can autonomously respawn a LIVE worker on
//! its OWN restart, bypassing the chokepoint entirely. A LEGACY session whose
//! worker carried no claude-side `reattachEnv` then comes back env-less; cctui
//! sees it as "alive" in the `list` roster and won't re-resume an alive worker,
//! so it routes to the default upstream and 401s until its next genuine
//! cold-resume (worker death + user reply). Observed: a session reported
//! `account_bound: true` by the server while its live worker had lost gateway
//! env after a claude-daemon autonomous respawn.
//!
//! This module owns the bounded/idempotent decision of WHICH live workers need a
//! forced re-resume through the chokepoint. It deliberately holds no I/O: the
//! caller (the claude adapter poll loop) supplies the observed liveness and the
//! server's `account_bound` verdict, and performs the kill + cold-resume when
//! [`HealTracker::should_heal`] returns `true`.
//!
//! ## Detection signal
//!
//! cctui records every worker `short` it dispatched WITH a resolved gateway env
//! via [`HealTracker::note_launched_with_env`] (called from the spawn / resume /
//! cold-resume chokepoint). A live, account-bound worker that is NOT in that set
//! was therefore brought up by something other than cctui — i.e. an autonomous
//! claude-daemon respawn — and may be env-less. That worker is a heal candidate.
//!
//! Pairing the launched-with-env set with the server's authoritative
//! `account_bound` verdict avoids thrashing non-account sessions (which need no
//! gateway env at all) and avoids healing workers cctui itself just launched
//! with env.
//!
//! ## Bounded / idempotent invariant
//!
//! Healing kills and cold-resumes a live worker, so it MUST NOT thrash:
//!
//! 1. **Cap.** Each session is healed at most [`MAX_HEAL_ATTEMPTS`] times for the
//!    lifetime of the daemon process. After the cap is hit the session is parked
//!    in `exhausted` and never re-triggers (a clear comment over a silent retry
//!    storm). A genuine cctui-driven relaunch that records the short via
//!    `note_launched_with_env` clears the counter, so a later autonomous respawn
//!    of the SAME session can be healed afresh.
//! 2. **State transition only.** `should_heal` returns `true` only on the
//!    env-less → needs-heal transition. Once a heal is dispatched for a short it
//!    is marked `in_flight` and won't re-trigger until the heal completes
//!    (success → recorded via `note_launched_with_env`; failure → released via
//!    [`HealTracker::note_heal_failed`], which also burns an attempt).
//!
//! Together these bound the worst case to `MAX_HEAL_ATTEMPTS` kill+resume cycles
//! per session per daemon lifetime, with at most one heal in flight per session.

use std::collections::HashMap;

/// Maximum forced re-resumes a single session may receive over the daemon's
/// lifetime before it is parked.
///
/// Small on purpose: one heal almost always suffices (the cold-resume re-seeds
/// env + `reattachEnv`); the extra retries only cover a transient failure
/// mid-heal. Resetting on a successful launched-with-env record means a
/// brand-new autonomous-respawn episode for the same session gets a fresh
/// budget.
pub const MAX_HEAL_ATTEMPTS: u8 = 3;

/// Per-session heal bookkeeping.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct HealState {
    /// Whether cctui has launched this worker WITH a resolved gateway env (or a
    /// heal is what most recently launched it). When `true` the worker is
    /// trusted to carry env and is never a heal candidate.
    launched_with_env: bool,
    /// Heals already dispatched for this session this daemon lifetime.
    attempts: u8,
    /// A heal is dispatched and not yet resolved. Suppresses re-triggering on
    /// the polls that elapse while the kill + cold-resume is in progress.
    in_flight: bool,
}

/// Bounded, idempotent tracker of which live workers need a forced gateway-env
/// re-resume (CCT-462). Holds no I/O; see the module docs for the invariant.
#[derive(Debug, Default)]
pub struct HealTracker {
    by_short: HashMap<String, HealState>,
}

impl HealTracker {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that cctui launched `short` WITH a resolved gateway env (spawn /
    /// resume / cold-resume / a completed heal). This is the trust signal: the
    /// worker carries env, so it stops being a heal candidate, and any prior
    /// heal-attempt budget is reset — a later autonomous respawn that drops the
    /// env can be healed afresh.
    pub fn note_launched_with_env(&mut self, short: &str) {
        let st = self.by_short.entry(short.to_owned()).or_default();
        st.launched_with_env = true;
        st.in_flight = false;
        st.attempts = 0;
    }

    /// Decide whether a live worker needs a forced re-resume.
    ///
    /// Returns `true` only on the env-less → needs-heal transition, and marks
    /// the session `in_flight` so it won't re-trigger until the heal resolves.
    /// The caller MUST, on the returned `true`, perform the kill + cold-resume
    /// and then call either [`Self::note_launched_with_env`] (success) or
    /// [`Self::note_heal_failed`] (failure).
    ///
    /// - `alive`: the worker is live in the roster (a dead/hibernated worker is
    ///   healed by the existing cold-resume-on-reply path, not here).
    /// - `account_bound`: the server reports this session bound to an OAuth
    ///   account, so it REQUIRES gateway env. Non-account sessions never heal.
    pub fn should_heal(&mut self, short: &str, alive: bool, account_bound: bool) -> bool {
        if !alive || !account_bound {
            return false;
        }
        let st = self.by_short.entry(short.to_owned()).or_default();
        // cctui launched it with env, or a heal is already in flight, or we've
        // exhausted the budget — none of those are the heal transition.
        if st.launched_with_env || st.in_flight || st.attempts >= MAX_HEAL_ATTEMPTS {
            return false;
        }
        st.in_flight = true;
        st.attempts = st.attempts.saturating_add(1);
        true
    }

    /// Release an in-flight heal that did NOT succeed (the kill + cold-resume
    /// errored). The attempt is already counted, so this just clears the
    /// in-flight latch; the next poll re-evaluates and retries until the cap.
    pub fn note_heal_failed(&mut self, short: &str) {
        if let Some(st) = self.by_short.get_mut(short) {
            st.in_flight = false;
        }
    }

    /// Drop a session that has left the live roster, so its bookkeeping doesn't
    /// leak for the daemon's lifetime. Idempotent.
    pub fn forget(&mut self, short: &str) {
        self.by_short.remove(short);
    }

    /// Whether `short` could conceivably need a heal right now — i.e. cctui has
    /// NOT launched it with env, no heal is in flight, and the budget isn't
    /// exhausted. A cheap, non-mutating pre-filter the caller uses to skip the
    /// server `account_bound` round-trip for the (common) trusted/parked
    /// workers; [`Self::should_heal`] remains the authoritative gate (it also
    /// requires the worker be live + account-bound). An unknown short is a
    /// candidate (default state has `launched_with_env: false`).
    #[must_use]
    pub fn is_candidate(&self, short: &str) -> bool {
        self.by_short.get(short).is_none_or(|st| {
            !st.launched_with_env && !st.in_flight && st.attempts < MAX_HEAL_ATTEMPTS
        })
    }

    /// Whether `short` has been parked after exhausting its heal budget — for
    /// caller-side logging only.
    #[must_use]
    pub fn is_exhausted(&self, short: &str) -> bool {
        self.by_short.get(short).is_some_and(|st| {
            !st.launched_with_env && !st.in_flight && st.attempts >= MAX_HEAL_ATTEMPTS
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{HealTracker, MAX_HEAL_ATTEMPTS};

    #[test]
    fn non_account_bound_never_heals() {
        let mut t = HealTracker::new();
        assert!(!t.should_heal("aaaa1111", true, false));
    }

    #[test]
    fn dead_worker_never_heals() {
        // A dead/hibernated worker is the cold-resume-on-reply path's job, not
        // ours — we only proactively heal LIVE workers (CCT-462).
        let mut t = HealTracker::new();
        assert!(!t.should_heal("aaaa1111", false, true));
    }

    #[test]
    fn env_less_account_bound_live_worker_heals_once_then_latches() {
        let mut t = HealTracker::new();
        // First sighting of an autonomously-respawned env-less worker → heal.
        assert!(t.should_heal("aaaa1111", true, true));
        // In flight: subsequent polls do NOT re-trigger while the kill +
        // cold-resume is running (idempotent on the env-less state).
        assert!(!t.should_heal("aaaa1111", true, true));
        assert!(!t.should_heal("aaaa1111", true, true));
    }

    #[test]
    fn launched_with_env_worker_is_never_a_candidate() {
        let mut t = HealTracker::new();
        t.note_launched_with_env("aaaa1111");
        assert!(!t.should_heal("aaaa1111", true, true));
    }

    #[test]
    fn successful_heal_clears_state_and_allows_future_episodes() {
        let mut t = HealTracker::new();
        assert!(t.should_heal("aaaa1111", true, true));
        // Heal completed: cold-resume re-seeded env, recorded as launched.
        t.note_launched_with_env("aaaa1111");
        assert!(!t.should_heal("aaaa1111", true, true));
        // A FRESH autonomous respawn would arrive as a new env-less sighting;
        // simulate it by forgetting (roster drop) then re-sighting. The budget
        // is fresh, so it heals again.
        t.forget("aaaa1111");
        assert!(t.should_heal("aaaa1111", true, true));
    }

    #[test]
    fn failed_heal_retries_up_to_the_cap_then_parks() {
        let mut t = HealTracker::new();
        for _ in 0..MAX_HEAL_ATTEMPTS {
            assert!(t.should_heal("aaaa1111", true, true), "should retry under the cap");
            t.note_heal_failed("aaaa1111");
        }
        // Budget exhausted: parked, never thrashes again this lifetime.
        assert!(!t.should_heal("aaaa1111", true, true));
        assert!(t.is_exhausted("aaaa1111"));
    }

    #[test]
    fn note_launched_resets_an_exhausted_budget() {
        let mut t = HealTracker::new();
        for _ in 0..MAX_HEAL_ATTEMPTS {
            t.should_heal("aaaa1111", true, true);
            t.note_heal_failed("aaaa1111");
        }
        assert!(t.is_exhausted("aaaa1111"));
        // A genuine cctui-driven relaunch (or eventual heal success) recorded
        // here resets the budget so the session isn't permanently parked.
        t.note_launched_with_env("aaaa1111");
        assert!(!t.is_exhausted("aaaa1111"));
        t.forget("aaaa1111");
        assert!(t.should_heal("aaaa1111", true, true));
    }
}
