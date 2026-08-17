//! Per-(account, provider) gateway rate limiting.
//!
//! Pay-per-token providers hand out an account-wide RPM/TPM tier that every
//! concurrent session shares; left unchecked, parallel dispatch starves the tier
//! or blows through it. The gateway proxies every request, so it is the natural
//! throttle point. Limits are an editable per-provider setting, default OFF (both
//! `None` ⇒ no limiting). Enforcement is an in-memory fixed 60s sliding window per
//! provider row — this is a single-instance server, so no shared store is needed.
//!
//! Requests count on admission; tokens count when a response's usage lands (the
//! same capture the metering path already reads), so TPM gates on the running
//! window total.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use uuid::Uuid;

use crate::state::AppState;

/// The rolling window RPM/TPM are measured over: requests-per-minute /
/// tokens-per-minute.
pub const RATE_WINDOW: Duration = Duration::from_mins(1);

/// Per-(account, provider) rate limits. Both optional; `None` ⇒ that dimension is
/// unlimited. Persisted as `{ "rpm": int?, "tpm": int? }` on the provider row.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RateLimits {
    /// Max requests admitted per rolling 60s window.
    pub rpm: Option<u32>,
    /// Max tokens counted per rolling 60s window.
    pub tpm: Option<u64>,
}

impl RateLimits {
    /// Parse the stored `rate_limits_json` blob. A zero / negative / missing value
    /// leaves that dimension unlimited, so an operator clears a limit by zeroing it.
    pub fn from_json(value: Option<&serde_json::Value>) -> Self {
        let obj = value.and_then(serde_json::Value::as_object);
        let positive = |key: &str| {
            obj.and_then(|o| o.get(key)).and_then(serde_json::Value::as_u64).filter(|&n| n > 0)
        };
        Self { rpm: positive("rpm").and_then(|n| u32::try_from(n).ok()), tpm: positive("tpm") }
    }

    /// Nothing to enforce ⇒ the proxy skips the window entirely.
    pub const fn is_unset(&self) -> bool {
        self.rpm.is_none() && self.tpm.is_none()
    }
}

/// One provider's rolling request + token samples, pruned to [`RATE_WINDOW`].
#[derive(Default)]
pub struct RateWindow {
    reqs: VecDeque<Instant>,
    tokens: VecDeque<(Instant, u64)>,
}

fn prune(window: &mut RateWindow, now: Instant) {
    while window.reqs.front().is_some_and(|&t| now.duration_since(t) >= RATE_WINDOW) {
        window.reqs.pop_front();
    }
    while window.tokens.front().is_some_and(|&(t, _)| now.duration_since(t) >= RATE_WINDOW) {
        window.tokens.pop_front();
    }
}

/// Seconds until `oldest` ages out of the window — the `Retry-After` hint. Rounds
/// up and never returns 0, so a caller that honours it is always past the sample.
fn retry_after_secs(oldest: Instant, now: Instant) -> i64 {
    let remaining = RATE_WINDOW.saturating_sub(now.duration_since(oldest));
    let secs = remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0);
    i64::try_from(secs).unwrap_or(i64::MAX).max(1)
}

/// Decide whether a request is admitted, recording it when it is. Returns
/// `Err(retry_after_secs)` when it would exceed either limit — TPM first (a
/// full token window is the pay-per-token concern), then RPM. A limit left
/// `None` never blocks.
pub fn admit(window: &mut RateWindow, limits: &RateLimits, now: Instant) -> Result<(), i64> {
    prune(window, now);
    if let Some(tpm) = limits.tpm {
        let used: u64 = window.tokens.iter().map(|&(_, t)| t).sum();
        if used >= tpm
            && let Some(&(oldest, _)) = window.tokens.front()
        {
            return Err(retry_after_secs(oldest, now));
        }
    }
    if let Some(rpm) = limits.rpm
        && u32::try_from(window.reqs.len()).unwrap_or(u32::MAX) >= rpm
        && let Some(&oldest) = window.reqs.front()
    {
        return Err(retry_after_secs(oldest, now));
    }
    window.reqs.push_back(now);
    Ok(())
}

/// Add a response's token count to the window, so the next [`admit`] gates TPM on
/// the running total. No-op for a zero count.
pub fn record_tokens(window: &mut RateWindow, tokens: u64, now: Instant) {
    if tokens == 0 {
        return;
    }
    prune(window, now);
    window.tokens.push_back((now, tokens));
}

/// Admit a request against the provider row's live window. `Ok(())` when
/// admitted; `Err(retry_after_secs)` when it would exceed a configured limit.
pub fn admit_request(state: &AppState, provider_id: Uuid, limits: &RateLimits) -> Result<(), i64> {
    let mut window = state.gateway_rate_windows.entry(provider_id).or_default();
    admit(&mut window, limits, Instant::now())
}

/// Record a response's token count against the provider row's live window, so the
/// next [`admit`] gates TPM on the running total.
pub fn note_tokens(windows: &DashMap<Uuid, RateWindow>, provider_id: Uuid, tokens: u64) {
    let mut window = windows.entry(provider_id).or_default();
    record_tokens(&mut window, tokens, Instant::now());
}

#[cfg(test)]
mod tests {
    use super::{RATE_WINDOW, RateLimits, RateWindow, admit, record_tokens};
    use std::time::{Duration, Instant};

    #[test]
    fn from_json_reads_positive_rpm_tpm_and_treats_zero_as_unset() {
        let full = RateLimits::from_json(Some(&serde_json::json!({ "rpm": 30, "tpm": 90_000 })));
        assert_eq!(full, RateLimits { rpm: Some(30), tpm: Some(90_000) });
        assert!(!full.is_unset());

        // Absent, zero, and null all leave the dimension unlimited.
        assert_eq!(RateLimits::from_json(None), RateLimits::default());
        assert!(RateLimits::from_json(None).is_unset());
        let zeroed = RateLimits::from_json(Some(&serde_json::json!({ "rpm": 0, "tpm": 0 })));
        assert!(zeroed.is_unset());
        let partial = RateLimits::from_json(Some(&serde_json::json!({ "rpm": 5 })));
        assert_eq!(partial, RateLimits { rpm: Some(5), tpm: None });
    }

    #[test]
    fn rpm_admits_up_to_the_limit_then_blocks_until_the_window_frees() {
        let limits = RateLimits { rpm: Some(3), tpm: None };
        let mut w = RateWindow::default();
        let t0 = Instant::now();
        // Three in the window succeed; the fourth 429s with a Retry-After.
        for _ in 0..3 {
            assert!(admit(&mut w, &limits, t0).is_ok());
        }
        let retry = admit(&mut w, &limits, t0).expect_err("4th in-window request must block");
        assert!(retry >= 1, "Retry-After must be a positive hint, got {retry}");

        // Still blocked just before the oldest sample ages out.
        let almost = (t0 + RATE_WINDOW).checked_sub(Duration::from_secs(1)).unwrap();
        assert!(admit(&mut w, &limits, almost).is_err());

        // Once the first sample leaves the window, a slot frees and the next
        // request is admitted.
        let after = t0 + RATE_WINDOW + Duration::from_millis(1);
        assert!(
            admit(&mut w, &limits, after).is_ok(),
            "request must succeed after the window frees"
        );
    }

    #[test]
    fn tpm_gates_on_the_running_token_total_and_frees_after_the_window() {
        let limits = RateLimits { rpm: None, tpm: Some(1_000) };
        let mut w = RateWindow::default();
        let t0 = Instant::now();
        // Under budget: admitted, then usage lands.
        assert!(admit(&mut w, &limits, t0).is_ok());
        record_tokens(&mut w, 1_000, t0);
        // At/over budget: the next request 429s until the token sample ages out.
        let retry = admit(&mut w, &limits, t0).expect_err("a full token window must block");
        assert!(retry >= 1);
        let after = t0 + RATE_WINDOW + Duration::from_millis(1);
        assert!(admit(&mut w, &limits, after).is_ok(), "tokens age out; request admitted");
    }

    #[test]
    fn unset_limits_never_throttle() {
        let limits = RateLimits::default();
        let mut w = RateWindow::default();
        let t0 = Instant::now();
        for _ in 0..10_000 {
            assert!(admit(&mut w, &limits, t0).is_ok());
        }
        record_tokens(&mut w, u64::from(u32::MAX), t0);
        assert!(admit(&mut w, &limits, t0).is_ok());
    }

    #[test]
    fn record_tokens_ignores_a_zero_count() {
        let mut w = RateWindow::default();
        record_tokens(&mut w, 0, Instant::now());
        // A zero sample must not gate a tpm=1 window.
        let limits = RateLimits { rpm: None, tpm: Some(1) };
        assert!(admit(&mut w, &limits, Instant::now()).is_ok());
    }
}
