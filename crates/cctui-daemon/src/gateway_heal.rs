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
//! sees it as "alive" in the `list` roster and won't re-resume an alive worker.
//! On k8s that worker 401s; on a DESKTOP daemon it is worse — it silently falls
//! back to the machine's ambient `~/.claude` login and bills whatever account
//! that is, invisible to the gateway (CCT-574). Either way it stays broken
//! until its next genuine cold-resume (worker death + user reply). Observed: a
//! session reported
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
//!
//! ## Env-delivery verification (CCT-574)
//!
//! Trust used to be recorded on dispatch `ok: true` alone — but delivery can
//! fail INSIDE the claude daemon (observed: a worker claimed from the pre-warmed
//! spare pool exec'd without the dispatch `env`, so an account-bound session ran
//! on the machine's ambient login while cctui believed it launched with env, and
//! the heal↔cold-resume cycle looped forever because every "successful" heal
//! re-recorded trust). So the poll loop now VERIFIES delivery: for a trusted
//! worker whose launch env carried a gateway token, it checks the live worker
//! process actually carries that token ([`HealTracker::verify_hash`] →
//! `/proc/<pid>/environ` on Linux). A confirmed miss ([`ENV_VERIFY_STRIKES`]
//! consecutive polls, [`HealTracker::note_env_missing`]) revokes trust so the
//! regular heal machinery retries delivery — but each confirmed miss also burns
//! a `delivery_failures` point that `note_launched_with_env` does NOT reset;
//! at [`MAX_DELIVERY_FAILURES`] the session is parked with one loud error
//! instead of thrashing. A verified delivery ([`HealTracker::note_env_observed`])
//! clears the budget.

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

/// Consecutive polls observing the live worker process WITHOUT its expected
/// gateway token before trust is revoked (CCT-574).
///
/// Two on purpose: a single miss could race the worker's exec or a mid-claim
/// respawn.
pub const ENV_VERIFY_STRIKES: u8 = 2;

/// Confirmed env-delivery failures before the session is PARKED (CCT-574).
///
/// A confirmed failure means the dispatch carried gateway env yet the worker
/// process demonstrably runs without it, and each one already cost a kill +
/// cold-resume retry. Past this cap the delivery path itself is broken (e.g.
/// the claude daemon's spare claim dropping dispatch env) and further kills
/// only churn the session while it consumes the machine's ambient login —
/// park it and say so loudly.
pub const MAX_DELIVERY_FAILURES: u8 = 3;

/// Outcome of an observed env-delivery miss (CCT-574); tells the caller what to
/// log. See [`HealTracker::note_env_missing`].
#[derive(Debug, PartialEq, Eq)]
pub enum EnvMissing {
    /// Below the strike threshold (or the worker isn't verifiable) — no action.
    Strike,
    /// Trust revoked: the worker provably lacks its gateway env. The regular
    /// env-less heal will kill + cold-resume it on the next poll.
    Revoked,
    /// Trust revoked AND the delivery budget is spent: parked, no more heals.
    /// The session is running on ambient credentials — log loudly.
    Exhausted,
}

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
    /// The launch env was VERIFIED to have reached the worker process
    /// (CCT-574). Once set, verification stops for this launch; reset on every
    /// launched-with-env record so each new launch is re-verified.
    env_verified: bool,
    /// Consecutive polls the worker process was observed without its expected
    /// token (CCT-574). Reset on launch and on a verified observation.
    env_verify_strikes: u8,
    /// Confirmed env-delivery failures for this session (CCT-574).
    /// Deliberately NOT reset by `note_launched_with_env` — the heal's own
    /// cold-resume records trust, and letting it clear this budget is exactly
    /// the infinite heal↔cold-resume loop this field exists to break. Cleared
    /// only by a verified delivery or a roster drop (`forget`).
    delivery_failures: u8,
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
        // Each launch is a fresh delivery to verify (CCT-574) — but
        // `delivery_failures` deliberately survives: the heal's own cold-resume
        // lands here, and resetting the budget would re-arm the infinite
        // heal↔cold-resume loop verification exists to break.
        st.env_verified = false;
        st.env_verify_strikes = 0;
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

    /// The token hash to VERIFY against the live worker process (CCT-574):
    /// recorded for a trusted worker whose launch env carried a token, not yet
    /// verified this launch, no heal in flight, delivery budget not spent.
    /// `None` otherwise — including hash-less trusted workers (non-account,
    /// grandfathered), which have nothing to verify.
    #[must_use]
    pub fn verify_hash(&self, short: &str) -> Option<String> {
        self.by_short.get(short).and_then(|st| {
            (st.launched_with_env
                && !st.env_verified
                && !st.in_flight
                && st.delivery_failures < MAX_DELIVERY_FAILURES)
                .then(|| st.token_hash.clone())
                .flatten()
        })
    }

    /// Record that the worker process was observed CARRYING its expected
    /// gateway token (CCT-574): delivery worked, stop verifying this launch,
    /// and clear the delivery-failure budget — the path is proven good again.
    pub fn note_env_observed(&mut self, short: &str) {
        if let Some(st) = self.by_short.get_mut(short) {
            st.env_verified = true;
            st.env_verify_strikes = 0;
            st.delivery_failures = 0;
        }
    }

    /// Record that the worker process was observed WITHOUT its expected gateway
    /// token (CCT-574). Strikes accumulate per poll; at [`ENV_VERIFY_STRIKES`]
    /// the miss is confirmed: trust is revoked (the regular env-less heal takes
    /// over next poll) and a `delivery_failures` point is burned. At
    /// [`MAX_DELIVERY_FAILURES`] the session parks instead — the heal budget is
    /// zeroed out so neither heal path fires again — and the caller must log
    /// the ambient-credentials fallout loudly.
    pub fn note_env_missing(&mut self, short: &str) -> EnvMissing {
        let Some(st) = self.by_short.get_mut(short) else { return EnvMissing::Strike };
        // Only a trusted, unverified worker accumulates strikes; anything else
        // is (or will be) the env-less heal path's business.
        if !st.launched_with_env || st.env_verified || st.token_hash.is_none() {
            return EnvMissing::Strike;
        }
        st.env_verify_strikes = st.env_verify_strikes.saturating_add(1);
        if st.env_verify_strikes < ENV_VERIFY_STRIKES {
            return EnvMissing::Strike;
        }
        // Confirmed: the dispatch claimed success but the process runs bare.
        st.launched_with_env = false;
        st.token_hash = None;
        st.env_verify_strikes = 0;
        st.delivery_failures = st.delivery_failures.saturating_add(1);
        if st.delivery_failures >= MAX_DELIVERY_FAILURES {
            // Park hard: exhaust the heal budget so `should_heal` /
            // `note_token_invalid` refuse from here on.
            st.attempts = MAX_HEAL_ATTEMPTS;
            return EnvMissing::Exhausted;
        }
        EnvMissing::Revoked
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
    use super::{
        ENV_VERIFY_STRIKES, EnvMissing, HealTracker, MAX_DELIVERY_FAILURES, MAX_HEAL_ATTEMPTS,
        STALE_TOKEN_STRIKES, sha256_hex,
    };

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

    // ---- env-delivery verification (CCT-574) ----

    #[test]
    fn verify_hash_stops_after_a_verified_delivery_and_rearms_on_relaunch() {
        let mut t = HealTracker::new();
        // Nothing recorded / hash-less launches: nothing to verify.
        assert_eq!(t.verify_hash("aaaa1111"), None);
        t.note_launched_with_env("aaaa1111", None);
        assert_eq!(t.verify_hash("aaaa1111"), None);
        // Token-carrying launch: verify until observed, then stop.
        t.note_launched_with_env("aaaa1111", Some("abc".into()));
        assert_eq!(t.verify_hash("aaaa1111"), Some("abc".into()));
        t.note_env_observed("aaaa1111");
        assert_eq!(t.verify_hash("aaaa1111"), None);
        // A relaunch is a fresh delivery — re-verify.
        t.note_launched_with_env("aaaa1111", Some("def".into()));
        assert_eq!(t.verify_hash("aaaa1111"), Some("def".into()));
    }

    #[test]
    fn confirmed_env_miss_revokes_trust_after_strikes() {
        let mut t = HealTracker::new();
        t.note_launched_with_env("aaaa1111", Some("abc".into()));
        // One miss could race the worker's exec — no action.
        for _ in 0..ENV_VERIFY_STRIKES - 1 {
            assert_eq!(t.note_env_missing("aaaa1111"), EnvMissing::Strike);
            assert!(!t.should_heal("aaaa1111", true, true), "still trusted below the threshold");
        }
        // The confirming miss revokes trust; the regular env-less heal fires.
        assert_eq!(t.note_env_missing("aaaa1111"), EnvMissing::Revoked);
        assert_eq!(t.verify_hash("aaaa1111"), None, "trust revoked, nothing to verify");
        assert!(t.should_heal("aaaa1111", true, true));
    }

    #[test]
    fn delivery_failures_survive_relaunch_and_park_at_the_cap() {
        // The v0.7.x heal loop (CCT-574): heal → cold-resume records trust →
        // delivery fails again → heal → … forever. The delivery budget must
        // survive `note_launched_with_env` and park the session at the cap.
        let mut t = HealTracker::new();
        for failure in 1..=MAX_DELIVERY_FAILURES {
            t.note_launched_with_env("aaaa1111", Some("abc".into()));
            for _ in 0..ENV_VERIFY_STRIKES - 1 {
                assert_eq!(t.note_env_missing("aaaa1111"), EnvMissing::Strike);
            }
            let verdict = t.note_env_missing("aaaa1111");
            if failure < MAX_DELIVERY_FAILURES {
                assert_eq!(verdict, EnvMissing::Revoked);
                // The env-less heal retries delivery (kill + cold-resume, which
                // will re-record trust at the top of the next iteration).
                assert!(t.should_heal("aaaa1111", true, true));
            } else {
                assert_eq!(verdict, EnvMissing::Exhausted);
            }
        }
        // Parked: untrusted but no heal fires — the loop is broken.
        assert!(!t.should_heal("aaaa1111", true, true));
        assert!(t.is_exhausted("aaaa1111"));
        // And verification stays off even if something re-records trust.
        t.note_launched_with_env("aaaa1111", Some("def".into()));
        assert_eq!(t.verify_hash("aaaa1111"), None, "spent delivery budget disables verification");
    }

    #[test]
    fn verified_delivery_clears_the_failure_budget() {
        let mut t = HealTracker::new();
        // One confirmed failure…
        t.note_launched_with_env("aaaa1111", Some("abc".into()));
        for _ in 0..ENV_VERIFY_STRIKES {
            t.note_env_missing("aaaa1111");
        }
        // …then the retry actually lands: budget resets to pristine.
        t.note_launched_with_env("aaaa1111", Some("def".into()));
        t.note_env_observed("aaaa1111");
        for failure in 1..=MAX_DELIVERY_FAILURES {
            t.note_launched_with_env("aaaa1111", Some("ghi".into()));
            for _ in 0..ENV_VERIFY_STRIKES - 1 {
                assert_eq!(t.note_env_missing("aaaa1111"), EnvMissing::Strike);
            }
            let verdict = t.note_env_missing("aaaa1111");
            assert_eq!(
                verdict,
                if failure < MAX_DELIVERY_FAILURES {
                    EnvMissing::Revoked
                } else {
                    EnvMissing::Exhausted
                },
                "a full budget must be available after a verified delivery"
            );
            if failure < MAX_DELIVERY_FAILURES {
                assert!(t.should_heal("aaaa1111", true, true));
            }
        }
    }

    #[test]
    fn env_missing_on_untrusted_or_verified_workers_is_inert() {
        let mut t = HealTracker::new();
        // Unknown short: inert.
        assert_eq!(t.note_env_missing("aaaa1111"), EnvMissing::Strike);
        // Verified launch: inert (no strikes accumulate).
        t.note_launched_with_env("aaaa1111", Some("abc".into()));
        t.note_env_observed("aaaa1111");
        for _ in 0..ENV_VERIFY_STRIKES * 2 {
            assert_eq!(t.note_env_missing("aaaa1111"), EnvMissing::Strike);
        }
        assert_eq!(t.verify_hash("aaaa1111"), None);
        assert!(!t.should_heal("aaaa1111", true, true), "still trusted");
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
