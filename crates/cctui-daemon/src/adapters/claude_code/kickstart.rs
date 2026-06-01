//! Self-heal the on-demand `claude daemon` (CCT-194).
//!
//! The claude supervisor runs *on demand*: `claude daemon` reports it "runs
//! on demand and exits when the last client disconnects", and its own status
//! line attributes a live instance to `origin: transient — started on-demand
//! by claude agents`. So after an idle period, laptop sleep, or a control-
//! socket teardown there is frequently **no** `control.sock` at all — every
//! `list` poll and every `dispatch` from this adapter then fails with "no
//! claude daemon socket present", and the user has to wake it by hand.
//!
//! We boot it with `claude daemon run` — the documented "Run the supervisor
//! in the foreground" entrypoint — spawned **detached** (its own process
//! group, stdin/stdout/stderr to /dev/null) and *not* awaited: it stays up as
//! the supervisor while we poll for its socket to appear.
//!
//! NB: `claude agents --json` is the wrong primitive (the original CCT-194
//! attempt). Despite "does not require a TTY", it is a read-only scripting
//! query that connects-or-returns and exits 0 *without* booting the daemon
//! when none is running — verified: socket stays absent. Only a client that
//! actually spins up the supervisor (the interactive `claude agents` TUI, or
//! `claude daemon run`) brings the socket up. We use `daemon run` because it
//! needs no TTY.

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

    /// Boot the on-demand `claude daemon` by spawning `claude daemon run`
    /// detached. Unless `force`, no-ops if a previous attempt was made within
    /// [`KICKSTART_MIN_INTERVAL`]. Best-effort: failures are logged, never
    /// propagated — a still-missing socket surfaces as the usual poll/dispatch
    /// error on the next attempt.
    ///
    /// The child is *not* awaited: `claude daemon run` is the supervisor
    /// itself and stays in the foreground for its whole life. We put it in its
    /// own process group so our signals don't reach it, and reap it from a
    /// detached task so it never lingers as a zombie once it does exit (e.g.
    /// when it idle-shuts-down). The caller polls for the socket to appear.
    ///
    /// Spawns and returns immediately (no `.await`); must be called from
    /// within a Tokio runtime so the reaper task can be spawned.
    pub(super) fn kick(&self, force: bool) {
        if !self.gate(Instant::now(), force) {
            return;
        }
        tracing::info!("no claude daemon socket — booting via `claude daemon run`");
        let mut cmd = tokio::process::Command::new(&self.claude_bin);
        cmd.args(["daemon", "run"])
            // `claude` lives in `~/.local/bin`, off launchd's minimal PATH
            // (CCT-138) — give the child an augmented PATH so exec succeeds.
            .env("PATH", crate::childenv::child_path())
            // No TTY, no stdin: `daemon run` logs to its own daemon.log; detach
            // every stdio so it never blocks on or inherits our handles.
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // Detach into its own process group so a SIGTERM/SIGINT to this daemon
        // (and Ctrl-C in a foreground run) doesn't tear the supervisor down.
        #[cfg(unix)]
        cmd.process_group(0);
        match cmd.spawn() {
            Ok(mut child) => {
                tracing::info!("spawned detached `claude daemon run` supervisor");
                tokio::spawn(async move {
                    match child.wait().await {
                        Ok(s) => {
                            tracing::info!(code = ?s.code(), "`claude daemon run` exited");
                        }
                        Err(err) => tracing::warn!(%err, "waiting on `claude daemon run`"),
                    }
                });
            }
            Err(err) => tracing::warn!(%err, "failed to spawn `claude daemon run`"),
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
