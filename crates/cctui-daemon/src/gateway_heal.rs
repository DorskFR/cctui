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
//!
//! ## Token-validity probing (CCT-462 finish)
//!
//! Launch-trust memory alone misses one live failure class: a TRUSTED worker
//! whose `session_tokens` row got unbound/deleted server-side keeps its env but
//! 401s forever at the gateway session-token stage — it is never a heal
//! candidate above, so it was never healed. So each trusted launch also records
//! the sha256 hex of the gateway token it carried
//! ([`HealTracker::note_launched_with_env`]'s `token_hash`), and the poll loop
//! runs a low-frequency sweep asking the server whether that hash still
//! resolves. `valid: false` confirmed [`STALE_TOKEN_STRIKES`] times in a row
//! ([`HealTracker::note_token_invalid`]) revokes the worker's trust and fires
//! the SAME bounded heal machinery (latch + attempt budget); a `valid: true`
//! ([`HealTracker::note_token_valid`]) resets the strikes. Probe errors are
//! "unknown", never invalid — the heal kill is destructive, so it fails open.

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

/// Consecutive `valid: false` verdicts from the server's token-validity probe
/// before a TRUSTED worker is healed (CCT-462).
///
/// Two on purpose: a single `false` could be a transient (e.g. a rebind racing
/// the probe), and the heal is a destructive kill + cold-resume; a token that
/// stays unresolvable across two sweeps (~2 min apart) is genuinely orphaned.
pub const STALE_TOKEN_STRIKES: u8 = 2;

/// Per-session heal bookkeeping.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct HealState {
    /// Whether cctui has launched this worker WITH a resolved gateway env (or a
    /// heal is what most recently launched it). When `true` the worker is
    /// trusted to carry env and is never an env-less heal candidate — but it IS
    /// a token-validity probe candidate when `token_hash` is recorded.
    launched_with_env: bool,
    /// sha256 hex of the gateway session token the worker was launched with
    /// (`ANTHROPIC_AUTH_TOKEN` / `OPENAI_API_KEY`), when the launch env carried
    /// one (CCT-462). Only the hash is kept — never token material — consistent
    /// with the no-token-on-disk invariant (CCT-503; this map is memory-only
    /// anyway). `None` for non-account launches and grandfathered workers,
    /// which are never validity-probed.
    token_hash: Option<String>,
    /// Consecutive `valid: false` probe verdicts (CCT-462). Reset on a `valid`
    /// verdict and on every launched-with-env record; a heal fires at
    /// [`STALE_TOKEN_STRIKES`].
    stale_token_strikes: u8,
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
    ///
    /// `token_hash` (CCT-462) is the sha256 hex of the gateway session token
    /// the launch env carried (`ANTHROPIC_AUTH_TOKEN` / `OPENAI_API_KEY`), when
    /// present — it arms the low-frequency token-validity probe for this
    /// trusted worker. `None` (non-account launch, grandfathered survivor)
    /// records trust without arming the probe.
    pub fn note_launched_with_env(&mut self, short: &str, token_hash: Option<String>) {
        let st = self.by_short.entry(short.to_owned()).or_default();
        st.launched_with_env = true;
        st.token_hash = token_hash;
        st.stale_token_strikes = 0;
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

    /// The recorded token hash for a TRUSTED worker that is currently
    /// probe-able (CCT-462): launched with env AND carrying a hash, no heal in
    /// flight, budget not exhausted. `None` for everything else — untrusted
    /// workers are the env-less heal's business, hash-less trusted workers
    /// (non-account, grandfathered) have nothing to probe.
    #[must_use]
    pub fn probe_hash(&self, short: &str) -> Option<String> {
        self.by_short.get(short).and_then(|st| {
            (st.launched_with_env && !st.in_flight && st.attempts < MAX_HEAL_ATTEMPTS)
                .then(|| st.token_hash.clone())
                .flatten()
        })
    }

    /// Record a `valid: true` probe verdict: the token still resolves, so any
    /// accumulated strikes were transient — reset them.
    pub fn note_token_valid(&mut self, short: &str) {
        if let Some(st) = self.by_short.get_mut(short) {
            st.stale_token_strikes = 0;
        }
    }

    /// Record a `valid: false` probe verdict (CCT-462). Returns `true` when the
    /// strike count reaches [`STALE_TOKEN_STRIKES`] and a heal should be
    /// dispatched NOW — in that case the worker's trust is revoked (it is about
    /// to be killed; the relaunch re-records a fresh hash via
    /// [`Self::note_launched_with_env`]), the session is latched `in_flight`,
    /// and an attempt is burned, exactly like [`Self::should_heal`]. The caller
    /// MUST then perform the kill + cold-resume and resolve the latch via
    /// `note_launched_with_env` (success) or [`Self::note_heal_failed`]
    /// (failure — the worker stays untrusted, so retries flow through the
    /// regular env-less heal path until the cap).
    ///
    /// Below the strike threshold, or with a heal in flight / the budget
    /// exhausted, returns `false` and heals nothing.
    pub fn note_token_invalid(&mut self, short: &str) -> bool {
        let Some(st) = self.by_short.get_mut(short) else { return false };
        // Only a trusted, probe-able worker accumulates strikes; anything else
        // is (or will be) handled by the env-less heal path.
        if !st.launched_with_env || st.token_hash.is_none() {
            return false;
        }
        st.stale_token_strikes = st.stale_token_strikes.saturating_add(1);
        if st.stale_token_strikes < STALE_TOKEN_STRIKES
            || st.in_flight
            || st.attempts >= MAX_HEAL_ATTEMPTS
        {
            return false;
        }
        // Confirmed stale: treat exactly like an env-less detection. Revoke
        // trust (the worker is about to be killed), latch, burn an attempt.
        st.launched_with_env = false;
        st.token_hash = None;
        st.stale_token_strikes = 0;
        st.in_flight = true;
        st.attempts = st.attempts.saturating_add(1);
        true
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

/// sha256 hex of a token string — the fingerprint recorded per trusted worker
/// and sent to the server's token-validity probe (CCT-462).
///
/// Matches the server's `session_tokens.token_hash` encoding (`sha256_hex`),
/// so hash equality is the resolvability check without token material on the
/// wire.
#[must_use]
pub fn sha256_hex(token: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(token.as_bytes());
    digest.iter().fold(String::with_capacity(64), |mut acc, b| {
        use std::fmt::Write;
        let _ = write!(acc, "{b:02x}");
        acc
    })
}

#[cfg(test)]
mod tests {
    use super::{HealTracker, MAX_HEAL_ATTEMPTS, STALE_TOKEN_STRIKES, sha256_hex};

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
        t.note_launched_with_env("aaaa1111", None);
        assert!(!t.should_heal("aaaa1111", true, true));
    }

    #[test]
    fn successful_heal_clears_state_and_allows_future_episodes() {
        let mut t = HealTracker::new();
        assert!(t.should_heal("aaaa1111", true, true));
        // Heal completed: cold-resume re-seeded env, recorded as launched.
        t.note_launched_with_env("aaaa1111", None);
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
        t.note_launched_with_env("aaaa1111", None);
        assert!(!t.is_exhausted("aaaa1111"));
        t.forget("aaaa1111");
        assert!(t.should_heal("aaaa1111", true, true));
    }

    // ---- token-validity probing (CCT-462 finish) ----

    #[test]
    fn probe_hash_only_for_trusted_workers_with_a_recorded_hash() {
        let mut t = HealTracker::new();
        // Unknown short: nothing to probe.
        assert_eq!(t.probe_hash("aaaa1111"), None);
        // Trusted without a hash (non-account launch / grandfathered): no probe.
        t.note_launched_with_env("aaaa1111", None);
        assert_eq!(t.probe_hash("aaaa1111"), None);
        // Trusted with a hash: probe-able.
        t.note_launched_with_env("aaaa1111", Some("abc123".into()));
        assert_eq!(t.probe_hash("aaaa1111"), Some("abc123".into()));
        // An untrusted (env-less candidate) worker is never probed — it's the
        // regular heal path's business.
        t.forget("aaaa1111");
        assert!(t.should_heal("aaaa1111", true, true));
        assert_eq!(t.probe_hash("aaaa1111"), None);
    }

    #[test]
    fn invalid_token_heals_only_after_consecutive_strikes() {
        let mut t = HealTracker::new();
        t.note_launched_with_env("aaaa1111", Some("abc123".into()));
        // One strike is not enough — could be a transient (rebind racing the
        // probe); the heal is a destructive kill.
        for _ in 0..STALE_TOKEN_STRIKES - 1 {
            assert!(!t.note_token_invalid("aaaa1111"));
        }
        // The confirming strike fires the heal, revokes trust, and latches.
        assert!(t.note_token_invalid("aaaa1111"));
        assert_eq!(t.probe_hash("aaaa1111"), None, "trust revoked, no more probing");
        // In flight: neither probe path nor env-less path re-triggers.
        assert!(!t.note_token_invalid("aaaa1111"));
        assert!(!t.should_heal("aaaa1111", true, true));
        // Heal success re-records a fresh hash and resets everything.
        t.note_launched_with_env("aaaa1111", Some("def456".into()));
        assert_eq!(t.probe_hash("aaaa1111"), Some("def456".into()));
    }

    #[test]
    fn valid_verdict_resets_strikes() {
        let mut t = HealTracker::new();
        t.note_launched_with_env("aaaa1111", Some("abc123".into()));
        for _ in 0..STALE_TOKEN_STRIKES - 1 {
            assert!(!t.note_token_invalid("aaaa1111"));
        }
        // The token resolves again (e.g. the binding was fixed): strikes clear,
        // so a later single `false` does not heal.
        t.note_token_valid("aaaa1111");
        for _ in 0..STALE_TOKEN_STRIKES - 1 {
            assert!(
                !t.note_token_invalid("aaaa1111"),
                "strikes must restart after a valid verdict"
            );
        }
    }

    #[test]
    fn stale_token_heal_failure_respects_the_attempt_cap() {
        let mut t = HealTracker::new();
        // First episode: confirmed-stale heal burns attempt 1 and revokes
        // trust; its failure releases the latch, leaving the worker on the
        // regular env-less retry path.
        t.note_launched_with_env("aaaa1111", Some("abc123".into()));
        for _ in 0..STALE_TOKEN_STRIKES - 1 {
            assert!(!t.note_token_invalid("aaaa1111"));
        }
        assert!(t.note_token_invalid("aaaa1111"));
        t.note_heal_failed("aaaa1111");
        // The remaining budget drains through `should_heal` (the worker is now
        // untrusted), then the session parks.
        for _ in 1..MAX_HEAL_ATTEMPTS {
            assert!(t.should_heal("aaaa1111", true, true));
            t.note_heal_failed("aaaa1111");
        }
        assert!(t.is_exhausted("aaaa1111"));
        assert!(!t.should_heal("aaaa1111", true, true));
        // Re-trusting with a hash at the cap still can't stale-heal past a
        // fresh budget — note_launched resets attempts, which IS the fresh
        // budget; confirm the interaction is a reset, not an overflow.
        t.note_launched_with_env("aaaa1111", Some("def456".into()));
        for _ in 0..STALE_TOKEN_STRIKES - 1 {
            assert!(!t.note_token_invalid("aaaa1111"));
        }
        assert!(t.note_token_invalid("aaaa1111"), "fresh budget after re-trust");
    }

    #[test]
    fn sha256_hex_matches_known_vector() {
        // Empty-string SHA-256 — pins the encoding (lowercase hex) to the same
        // fingerprint the server stores in `session_tokens.token_hash`.
        assert_eq!(
            sha256_hex(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
