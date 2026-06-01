//! Self-heal the on-demand `claude daemon` (CCT-194).
//!
//! The claude supervisor runs *on demand*: `claude daemon` reports it "runs
//! on demand and exits when the last client disconnects", and its own status
//! line attributes a live instance to `origin: transient — started on-demand
//! by claude agents`. So after an idle period, laptop sleep, or a control-
//! socket teardown there is frequently **no** `control.sock` at all — every
//! `list` poll and every `dispatch` from this adapter then fails with "no
//! claude daemon socket present", and the user has to run `claude agents`
//! by hand to wake it.
//!
//! `claude agents --json` boots the supervisor and exits without needing a
//! TTY ("Print live sessions as a JSON array and exit (for scripting; does
//! not require a TTY)"), so it is the right primitive to kick the daemon
//! awake. [`Kickstarter`] runs it (rate-limited, so a missing socket in the
//! 2s poll loop doesn't spawn a fresh `claude` every tick) whenever the
//! socket is absent, so the daemon self-heals instead of requiring manual
//! intervention.

use std::process::Stdio;
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
    pub(super) fn new(claude_bin: String) -> Self {
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

    /// Boot the on-demand `claude daemon` via `claude agents --json`. Unless
    /// `force`, no-ops if a previous attempt was made within
    /// [`KICKSTART_MIN_INTERVAL`]. Best-effort: failures are logged, never
    /// propagated — a still-missing socket surfaces as the usual poll/dispatch
    /// error on the next attempt.
    pub(super) async fn kick(&self, force: bool) {
        if !self.gate(Instant::now(), force) {
            return;
        }
        tracing::info!("no claude daemon socket — kickstarting via `claude agents --json`");
        let res = tokio::process::Command::new(&self.claude_bin)
            .args(["agents", "--json"])
            // `claude` lives in `~/.local/bin`, off launchd's minimal PATH
            // (CCT-138) — give the child an augmented PATH so exec succeeds.
            .env("PATH", crate::childenv::child_path())
            // No TTY, no stdin: `--json` prints-and-exits; closing stdin keeps
            // it from ever blocking on input.
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
        match res {
            Ok(s) if s.success() => tracing::info!("claude daemon kickstart complete"),
            Ok(s) => tracing::warn!(code = ?s.code(), "`claude agents --json` exited non-zero"),
            Err(err) => tracing::warn!(%err, "failed to spawn `claude agents --json`"),
        }
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
