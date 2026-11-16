use std::time::Duration;

use cctui_proto::backoff::Backoff;

/// Sized so the doubling schedule still spans over two hours: a target
/// unreachable for an hour must recover, not dead-letter.
pub const MAX_ATTEMPTS: i32 = 11;
pub const CLAIM_LEASE_SECS: i64 = 300;
pub const CLAIM_BATCH: i64 = 50;

#[must_use]
pub const fn retry_schedule() -> Backoff {
    Backoff::new(Duration::from_secs(10), Duration::from_hours(1))
}

#[derive(Debug, PartialEq, Eq)]
pub enum Retry {
    After { attempts: i32, secs: i64 },
    Dead { attempts: i32 },
}

/// The transition after a failed attempt, `attempts` being the count before it.
#[must_use]
pub fn retry_after_failure(attempts: i32) -> Retry {
    let next = attempts + 1;
    if next >= MAX_ATTEMPTS {
        return Retry::Dead { attempts: next };
    }
    let secs = retry_schedule().delay_secs_for(u32::try_from(attempts).unwrap_or(u32::MAX));
    Retry::After { attempts: next, secs }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn retry_secs(attempts: i32) -> i64 {
        match retry_after_failure(attempts) {
            Retry::After { attempts: next, secs } => {
                assert_eq!(next, attempts + 1);
                secs
            }
            other @ Retry::Dead { .. } => panic!("attempts {attempts} should retry, got {other:?}"),
        }
    }

    #[test]
    fn backoff_doubles_from_ten_seconds_within_jitter() {
        for (attempts, base) in [(0, 10), (1, 20), (2, 40), (3, 80), (6, 640), (8, 2560)] {
            let secs = retry_secs(attempts);
            let (lo, hi) = (base * 8 / 10, base * 12 / 10);
            assert!((lo..=hi).contains(&secs), "attempts {attempts}: {secs}s outside {lo}..={hi}");
        }
    }

    #[test]
    fn backoff_caps_at_one_hour() {
        let secs = retry_secs(9);
        assert!((2880..=4320).contains(&secs), "attempts 9: {secs}s is not the 1 h cap");
        assert_eq!(retry_schedule().peek(), Duration::from_secs(10));
    }

    #[test]
    fn the_eleventh_failure_dead_letters() {
        for attempts in 0..MAX_ATTEMPTS - 1 {
            retry_secs(attempts);
        }
        assert_eq!(retry_after_failure(10), Retry::Dead { attempts: 11 });
        assert_eq!(retry_after_failure(20), Retry::Dead { attempts: 21 });
    }
}
