//! Bounding a policy check in time. A notification left unanswered blocks the
//! exec forever, so a check that hangs or crashes must still yield a denial.

use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use crate::engine::Verdict;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Allow,
    Deny(String),
}

impl From<Verdict> for Outcome {
    fn from(verdict: Verdict) -> Self {
        match verdict {
            Verdict::Allow => Self::Allow,
            Verdict::Deny { reason } | Verdict::Ask { reason } => Self::Deny(reason),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    TimedOut,
    Crashed,
    NoThread,
}

impl Failure {
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::TimedOut => "the guard did not decide in time (failing closed)",
            Self::Crashed => "the guard's policy check failed (failing closed)",
            Self::NoThread => "the guard could not start a policy check (failing closed)",
        }
    }
}

/// Run `check` on its own thread and wait at most `timeout` for it. A check
/// still running at the deadline is abandoned.
pub fn within<T, F>(timeout: Duration, check: F) -> Result<T, Failure>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (tx, rx) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("guard-exec-check".into())
        .spawn(move || {
            let _ = tx.send(check());
        })
        .map_err(|_| Failure::NoThread)?;
    rx.recv_timeout(timeout).map_err(|e| match e {
        RecvTimeoutError::Timeout => Failure::TimedOut,
        RecvTimeoutError::Disconnected => Failure::Crashed,
    })
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    #[test]
    fn a_prompt_check_returns_its_value() {
        assert_eq!(within(Duration::from_secs(5), || Outcome::Allow), Ok(Outcome::Allow));
    }

    #[test]
    fn a_hung_check_times_out_promptly() {
        let start = Instant::now();
        let got = within(Duration::from_millis(50), || {
            std::thread::sleep(Duration::from_secs(5));
            Outcome::Allow
        });
        assert_eq!(got, Err(Failure::TimedOut));
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn a_panicking_check_is_a_crash() {
        let got: Result<Outcome, _> = within(Duration::from_secs(5), || panic!("boom"));
        assert_eq!(got, Err(Failure::Crashed));
    }

    #[test]
    fn every_failure_reason_says_it_fails_closed() {
        for f in [Failure::TimedOut, Failure::Crashed, Failure::NoThread] {
            assert!(f.reason().contains("failing closed"), "{f:?}");
        }
    }

    #[test]
    fn ask_verdicts_deny_at_exec_level() {
        assert_eq!(Outcome::from(Verdict::ask("confirm")), Outcome::Deny("confirm".into()));
        assert_eq!(Outcome::from(Verdict::deny("no")), Outcome::Deny("no".into()));
        assert_eq!(Outcome::from(Verdict::Allow), Outcome::Allow);
    }
}
