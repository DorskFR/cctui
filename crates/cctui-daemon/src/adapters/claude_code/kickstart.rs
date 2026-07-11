//! Self-heal the on-demand `claude daemon` (CCT-194, CCT-590).
//!
//! The claude supervisor runs *on demand*: after an idle period, laptop sleep,
//! or a control-socket teardown there is frequently **no** `control.sock` at
//! all — every `list` poll and every `dispatch` from this adapter then fails
//! with "no claude daemon socket present".
//!
//! Rather than spawn `claude daemon run` as our own child (which coupled its
//! lifetime to cctui-daemon and left `Z <defunct>` zombies when the in-runtime
//! reaper missed the exit — CCT-590), we ensure the supervisor is installed
//! and running under the OS user service manager (see [`super::claude_service`]).
//! The service manager parents and reaps it; we only ever poll for its socket.

use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Minimum spacing between kickstart attempts. The poll loop runs every ~2s;
/// booting `claude` that often would be wasteful, and the supervisor takes a
/// moment to come up — so back off between unforced attempts.
const KICKSTART_MIN_INTERVAL: Duration = Duration::from_secs(15);

/// Rate-limited launcher for the on-demand `claude daemon`.
pub(super) struct Kickstarter {
    claude_bin: String,
    last: Mutex<Option<Instant>>,
}

impl Kickstarter {
    pub(super) const fn new(claude_bin: String) -> Self {
        Self { claude_bin, last: Mutex::new(None) }
    }

    /// Gate one attempt: record `now` and report whether enough time has
    /// elapsed since the previous attempt. `force` always permits (and still
    /// records the timestamp). Pure — unit-tested without spawning anything.
    fn gate(&self, now: Instant, force: bool) -> bool {
        let mut last = self.last.lock().unwrap_or_else(PoisonError::into_inner);
        let permit = force || last.is_none_or(|t| now.duration_since(t) >= KICKSTART_MIN_INTERVAL);
        if permit {
            *last = Some(now);
        }
        permit
    }

    /// Ensure the managed `claude daemon` service is installed and running.
    /// Unless `force`, no-ops if a previous attempt was made within
    /// [`KICKSTART_MIN_INTERVAL`]. Best-effort: failures are logged, never
    /// propagated — a still-missing socket surfaces as the usual poll/dispatch
    /// error on the next attempt.
    ///
    /// [`super::claude_service::ensure`] shells the OS service manager, so it
    /// runs on a blocking pool; must be called from within a Tokio runtime.
    /// Returns immediately (no `.await`) — the caller polls for the socket.
    pub(super) fn kick(&self, force: bool) {
        if !self.gate(Instant::now(), force) {
            return;
        }
        let claude_bin = self.claude_bin.clone();
        tokio::task::spawn_blocking(move || match super::claude_service::ensure(&claude_bin) {
            Ok(()) => tracing::debug!("managed claude daemon service ensured running"),
            Err(err) => tracing::warn!(%err, "failed to ensure managed claude daemon service"),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_permits_first_then_backs_off() {
        let k = Kickstarter::new("claude".into());
        let t0 = Instant::now();
        // First unforced attempt always permitted.
        assert!(k.gate(t0, false));
        // A second attempt within the window is denied.
        assert!(!k.gate(t0 + Duration::from_secs(1), false));
        // Once the window elapses, permitted again.
        assert!(k.gate(t0 + KICKSTART_MIN_INTERVAL, false));
    }

    #[test]
    fn gate_force_always_permits_and_records() {
        let k = Kickstarter::new("claude".into());
        let t0 = Instant::now();
        assert!(k.gate(t0, true));
        // Forced again immediately: still permitted...
        assert!(k.gate(t0 + Duration::from_millis(1), true));
        // ...and the forced attempt reset the clock, so an unforced one right
        // after is denied.
        assert!(!k.gate(t0 + Duration::from_millis(2), false));
    }
}
