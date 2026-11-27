//! Provider usage endpoints rate-limit per access token: every caller shares one
//! gate per account, whatever the number of sessions.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use uuid::Uuid;

pub const READ_INTERVAL: Duration = Duration::from_mins(1);
const BACKOFF_BASE: Duration = Duration::from_mins(3);
const BACKOFF_CAP: Duration = Duration::from_mins(30);
const RETRY_AFTER_CAP: Duration = Duration::from_hours(1);

#[derive(Default)]
struct Slot {
    open_at: Option<Instant>,
    failures: u32,
    retry_after: Option<Duration>,
}

#[derive(Default)]
pub struct ReadGate {
    slots: DashMap<Uuid, Slot>,
}

impl ReadGate {
    pub fn global() -> &'static Self {
        static GATE: OnceLock<ReadGate> = OnceLock::new();
        GATE.get_or_init(Self::default)
    }

    /// Reserve the account's next read, or return how long until one is allowed.
    ///
    /// # Errors
    /// The time left before the gate opens.
    pub fn claim(&self, account: Uuid, now: Instant) -> Result<(), Duration> {
        let mut slot = self.slots.entry(account).or_default();
        let result = match slot.open_at {
            Some(open_at) if open_at > now => Err(open_at - now),
            _ => {
                slot.open_at = Some(now + READ_INTERVAL);
                Ok(())
            }
        };
        drop(slot);
        result
    }

    pub fn succeeded(&self, account: Uuid, now: Instant) {
        self.slots.insert(
            account,
            Slot { open_at: Some(now + READ_INTERVAL), failures: 0, retry_after: None },
        );
    }

    /// Upstream's `Retry-After`, applied by the next [`Self::failed`].
    pub fn retry_after(&self, account: Uuid, wait: Duration) {
        self.slots.entry(account).or_default().retry_after = Some(wait);
    }

    /// Close the gate after a failed read; returns how long it stays closed.
    #[must_use]
    pub fn failed(&self, account: Uuid, now: Instant) -> Duration {
        let mut slot = self.slots.entry(account).or_default();
        slot.failures = slot.failures.saturating_add(1);
        let backoff = slot.retry_after.take().map_or_else(
            || exponential(slot.failures),
            |wait| wait.clamp(READ_INTERVAL, RETRY_AFTER_CAP),
        );
        slot.open_at = Some(now + backoff);
        backoff
    }
}

fn exponential(failures: u32) -> Duration {
    BACKOFF_BASE.saturating_mul(1 << failures.saturating_sub(1).min(8)).min(BACKOFF_CAP)
}

/// `Retry-After` as delta-seconds or an HTTP date.
#[must_use]
pub fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let value = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?.trim();
    if let Ok(secs) = value.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    let at = chrono::DateTime::parse_from_rfc2822(value).ok()?;
    (at.with_timezone(&chrono::Utc) - chrono::Utc::now()).to_std().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_read_per_interval_however_many_callers() {
        let gate = ReadGate::default();
        let (id, t0) = (Uuid::new_v4(), Instant::now());
        assert_eq!(gate.claim(id, t0), Ok(()));
        for _ in 0..50 {
            assert!(gate.claim(id, t0 + Duration::from_secs(1)).is_err());
        }
        gate.succeeded(id, t0);
        assert!(
            gate.claim(id, (t0 + READ_INTERVAL).checked_sub(Duration::from_secs(1)).unwrap())
                .is_err()
        );
        assert_eq!(gate.claim(id, t0 + READ_INTERVAL), Ok(()));
    }

    #[test]
    fn failures_back_off_exponentially_up_to_the_cap() {
        let gate = ReadGate::default();
        let (id, t0) = (Uuid::new_v4(), Instant::now());
        let waits: Vec<_> = (0..6)
            .map(|_| {
                gate.claim(id, t0 + Duration::from_hours(5)).ok();
                gate.failed(id, t0).as_secs() / 60
            })
            .collect();
        assert_eq!(waits, [3, 6, 12, 24, 30, 30]);
        assert_eq!(gate.claim(id, t0 + Duration::from_mins(29)), Err(Duration::from_mins(1)));
    }

    #[test]
    fn retry_after_sets_the_backoff_within_bounds() {
        let gate = ReadGate::default();
        let (id, t0) = (Uuid::new_v4(), Instant::now());
        gate.retry_after(id, Duration::from_mins(10));
        assert_eq!(gate.failed(id, t0), Duration::from_mins(10));
        gate.retry_after(id, Duration::from_secs(1));
        assert_eq!(gate.failed(id, t0), READ_INTERVAL);
        gate.retry_after(id, Duration::from_hours(9));
        assert_eq!(gate.failed(id, t0), RETRY_AFTER_CAP);
    }

    #[test]
    fn a_success_resets_the_backoff() {
        let gate = ReadGate::default();
        let (id, t0) = (Uuid::new_v4(), Instant::now());
        let _ = gate.failed(id, t0);
        let _ = gate.failed(id, t0);
        gate.succeeded(id, t0);
        assert_eq!(gate.failed(id, t0), BACKOFF_BASE);
    }

    #[test]
    fn retry_after_parses_seconds_and_dates() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "120".parse().unwrap());
        assert_eq!(parse_retry_after(&headers), Some(Duration::from_mins(2)));
        let at = (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc2822();
        headers.insert(reqwest::header::RETRY_AFTER, at.parse().unwrap());
        let parsed = parse_retry_after(&headers).unwrap();
        assert!(parsed > Duration::from_mins(4) && parsed <= Duration::from_mins(5));
        headers.insert(reqwest::header::RETRY_AFTER, "soon".parse().unwrap());
        assert_eq!(parse_retry_after(&headers), None);
    }
}
