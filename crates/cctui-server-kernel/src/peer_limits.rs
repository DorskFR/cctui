//! Per-sender rate ceilings shared by peer messaging, rooms and federation.
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use dashmap::DashMap;

/// Per-sender ceilings. A peer message is a turn in somebody else's session:
/// cheap to send, expensive to receive.
pub const SEND_PER_MIN: usize = 10;
pub const HISTORY_PER_MIN: usize = 30;
pub const WINDOW: Duration = Duration::from_mins(1);

/// A peer message becomes a prompt, so it is capped like one.
///
/// A room broadcast is the same message to several targets, so it is capped and
/// counted the same: one broadcast spends one send from the caller's window.
pub const MAX_MESSAGE_BYTES: usize = 32 * 1024;

/// Sliding-window counter, one window per key. The second field counts admits
/// so sweeps can be amortised over them.
#[derive(Default)]
pub struct Limiter(DashMap<String, VecDeque<Instant>>, std::sync::atomic::AtomicU64);

/// Keys this map may hold before a sweep is forced. Device-login keys come from
/// unauthenticated callers, so the key space is attacker-influenced and the map
/// has to be bounded by something other than good behaviour.
const MAX_KEYS: usize = 10_000;

/// How many admits pass between opportunistic sweeps. There is no timer thread:
/// the map only grows when it is used, so cleaning on use is enough.
const SWEEP_EVERY: u64 = 256;

impl Limiter {
    /// Record a call and report whether it is within `max` per [`WINDOW`].
    pub fn admit(&self, key: &str, max: usize, now: Instant) -> bool {
        let admitted = {
            let mut hits = self.0.entry(key.to_owned()).or_default();
            while hits.front().is_some_and(|t| now.duration_since(*t) >= WINDOW) {
                hits.pop_front();
            }
            if hits.len() >= max {
                false
            } else {
                hits.push_back(now);
                true
            }
        };
        self.maybe_sweep(now);
        admitted
    }

    /// Drop every key whose window has fully elapsed: an idle key carries no
    /// information, so keeping it only leaks memory.
    pub fn sweep(&self, now: Instant) {
        self.0.retain(|_, hits| hits.back().is_some_and(|t| now.duration_since(*t) < WINDOW));
    }

    #[cfg(any(test, feature = "test-support"))]
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    fn maybe_sweep(&self, now: Instant) {
        let due =
            self.1.fetch_add(1, std::sync::atomic::Ordering::Relaxed).is_multiple_of(SWEEP_EVERY);
        if !due && self.0.len() <= MAX_KEYS {
            return;
        }
        self.sweep(now);
        // A flood inside one window can outrun the sweep; the counters are a
        // throttle, not an audit, so dropping them beats unbounded growth.
        if self.0.len() > MAX_KEYS {
            self.0.clear();
        }
    }
}

#[must_use]
pub fn limiter() -> &'static Limiter {
    static LIMITER: std::sync::LazyLock<Limiter> = std::sync::LazyLock::new(Limiter::default);
    &LIMITER
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_admits_up_to_the_cap_then_refuses_until_it_slides() {
        let limiter = Limiter::default();
        let t0 = Instant::now();
        for i in 0..SEND_PER_MIN {
            assert!(limiter.admit("send:a", SEND_PER_MIN, t0), "call {i} must be admitted");
        }
        assert!(!limiter.admit("send:a", SEND_PER_MIN, t0), "the cap must hold");
        assert!(limiter.admit("send:b", SEND_PER_MIN, t0), "the window is per sender");
        assert!(
            limiter.admit("send:a", SEND_PER_MIN, t0 + WINDOW + Duration::from_secs(1)),
            "the window must slide"
        );
    }

    #[test]
    fn send_and_history_windows_are_counted_separately() {
        let limiter = Limiter::default();
        let t0 = Instant::now();
        for _ in 0..SEND_PER_MIN {
            assert!(limiter.admit("send:a", SEND_PER_MIN, t0));
        }
        assert!(!limiter.admit("send:a", SEND_PER_MIN, t0));
        assert!(limiter.admit("history:a", HISTORY_PER_MIN, t0));
    }
}
