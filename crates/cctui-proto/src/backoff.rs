//! Doubling reconnect backoff with ±20% jitter so peers don't retry in lockstep.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio_util::sync::CancellationToken;

const JITTER: f64 = 0.2;

#[derive(Debug, Clone)]
pub struct Backoff {
    min: Duration,
    max: Duration,
    cur: Duration,
    attempt: u32,
}

impl Backoff {
    #[must_use]
    pub const fn new(min: Duration, max: Duration) -> Self {
        Self { min, max, cur: min, attempt: 0 }
    }

    /// The 5 s → 60 s schedule shared by daemon and dispatcher WS reconnects.
    #[must_use]
    pub const fn reconnect() -> Self {
        Self::new(Duration::from_secs(5), Duration::from_mins(1))
    }

    #[must_use]
    pub const fn attempt(&self) -> u32 {
        self.attempt
    }

    /// Un-jittered delay the next [`Self::next_delay`] is centred on.
    #[must_use]
    pub const fn peek(&self) -> Duration {
        self.cur
    }

    pub fn next_delay(&mut self) -> Duration {
        let base = self.cur;
        self.cur = base.saturating_mul(2).min(self.max);
        self.attempt = self.attempt.saturating_add(1);
        jitter(base, unit_random())
    }

    pub const fn reset(&mut self) {
        self.cur = self.min;
        self.attempt = 0;
    }

    /// Jumps straight to the cap, for failures that won't clear soon.
    pub const fn saturate(&mut self) {
        self.cur = self.max;
    }

    /// Sleeps for [`Self::next_delay`]; `false` if `cancel` fired first.
    pub async fn sleep(&mut self, cancel: &CancellationToken) -> bool {
        let delay = self.next_delay();
        tokio::select! {
            () = tokio::time::sleep(delay) => true,
            () = cancel.cancelled() => false,
        }
    }
}

fn jitter(base: Duration, unit: f64) -> Duration {
    base.mul_f64(JITTER.mul_add(unit.mul_add(2.0, -1.0), 1.0))
}

/// Cheap splitmix64 over time + a counter; jitter needs spread, not quality.
#[allow(clippy::cast_precision_loss)]
fn unit_random() -> f64 {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
    let mut z = u64::from(nanos) ^ COUNTER.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn within(d: Duration, base: Duration) -> bool {
        d >= base.mul_f64(1.0 - JITTER) && d <= base.mul_f64(1.0 + JITTER)
    }

    #[test]
    fn doubles_caps_and_resets() {
        let mut b = Backoff::reconnect();
        let expected = [5, 10, 20, 40, 60, 60, 60];
        for secs in expected {
            assert_eq!(b.peek(), Duration::from_secs(secs));
            let d = b.next_delay();
            assert!(within(d, Duration::from_secs(secs)), "{d:?} vs {secs}s");
        }
        assert_eq!(b.attempt(), 7);
        b.reset();
        assert_eq!(b.peek(), Duration::from_secs(5));
        assert_eq!(b.attempt(), 0);
        b.saturate();
        assert_eq!(b.peek(), Duration::from_mins(1));
    }

    #[test]
    fn jitter_stays_within_bounds() {
        let base = Duration::from_secs(10);
        assert_eq!(jitter(base, 0.0), Duration::from_secs(8));
        assert_eq!(jitter(base, 1.0), Duration::from_secs(12));
        assert_eq!(jitter(base, 0.5), base);
        for _ in 0..1000 {
            let u = unit_random();
            assert!((0.0..1.0).contains(&u));
            assert!(within(jitter(base, u), base));
        }
    }

    #[test]
    fn jitter_spreads_values() {
        let first = unit_random();
        assert!((0..100).any(|_| (unit_random() - first).abs() > f64::EPSILON));
    }

    #[tokio::test]
    async fn sleep_returns_false_when_cancelled() {
        let cancel = CancellationToken::new();
        cancel.cancel();
        let mut b = Backoff::reconnect();
        let started = std::time::Instant::now();
        assert!(!b.sleep(&cancel).await);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test(start_paused = true)]
    async fn sleep_completes_when_not_cancelled() {
        let cancel = CancellationToken::new();
        let mut b = Backoff::new(Duration::from_millis(10), Duration::from_millis(40));
        assert!(b.sleep(&cancel).await);
        assert_eq!(b.peek(), Duration::from_millis(20));
    }
}
