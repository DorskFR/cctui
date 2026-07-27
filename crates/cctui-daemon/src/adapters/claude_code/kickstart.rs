//! Self-heal the on-demand `claude daemon`.
//!
//! The claude supervisor runs *on demand*: after an idle period, laptop sleep,
//! or a control-socket teardown there is frequently **no** `control.sock` at
//! all — every `list` poll and every `dispatch` from this adapter then fails
//! with "no claude daemon socket present".
//!
//! Rather than spawn `claude daemon run` as our own child (which coupled its
//! lifetime to cctui-daemon and left `Z <defunct>` zombies when the in-runtime
//! reaper missed the exit), we ensure the supervisor is installed
//! and running under the OS user service manager (see [`super::claude_service`]).
//! The service manager parents and reaps it; we only ever poll for its socket.
//!
//! Environments with **no usable service manager** — dispatched worker
//! containers foremost, which have no systemd and no user bus —
//! fall back to spawning `claude daemon run` as a direct detached child. The
//! worker contract (ephemeral, supervised,
//! `--no-auto-update`) never wants a resident OS service anyway. The fallback
//! also engages when [`super::claude_service::ensure`] itself fails (e.g.
//! `systemctl` present but the user bus unreachable), so a missed heuristic
//! still boots the daemon rather than timing out every dispatch.

use std::process::Stdio;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Minimum spacing between kickstart attempts. The poll loop runs every ~2s;
/// booting `claude` that often would be wasteful, and the supervisor takes a
/// moment to come up — so back off between unforced attempts.
const KICKSTART_MIN_INTERVAL: Duration = Duration::from_secs(15);

#[derive(Debug, PartialEq, Eq)]
enum Booted {
    Managed,
    Direct,
    DirectFallback,
}

fn boot(
    manager_available: bool,
    ensure: impl FnOnce() -> anyhow::Result<()>,
    spawn_direct: impl FnOnce(),
) -> Booted {
    if !manager_available {
        spawn_direct();
        return Booted::Direct;
    }
    match ensure() {
        Ok(()) => Booted::Managed,
        Err(err) => {
            tracing::warn!(
                %err,
                "failed to ensure managed claude daemon service; falling back to direct spawn"
            );
            spawn_direct();
            Booted::DirectFallback
        }
    }
}

/// Rate-limited launcher for the on-demand `claude daemon`.
pub(super) struct Kickstarter {
    claude_bin: String,
    manager_available: bool,
    last: Mutex<Option<Instant>>,
}

impl Kickstarter {
    pub(super) fn new(claude_bin: String) -> Self {
        Self::with_manager(claude_bin, super::claude_service::manager_available())
    }

    const fn with_manager(claude_bin: String, manager_available: bool) -> Self {
        Self { claude_bin, manager_available, last: Mutex::new(None) }
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

    /// Ensure the `claude daemon` is running: via the managed OS service on
    /// real hosts, or as a direct detached child where no service manager is
    /// usable (containers) or the managed path fails. Unless `force`, no-ops
    /// if a previous attempt was made within [`KICKSTART_MIN_INTERVAL`].
    /// Best-effort: failures are logged, never propagated — a still-missing
    /// socket surfaces as the usual poll/dispatch error on the next attempt.
    ///
    /// [`super::claude_service::ensure`] shells the OS service manager, so it
    /// runs on a blocking pool; must be called from within a Tokio runtime.
    /// Returns immediately (no `.await`) — the caller polls for the socket.
    pub(super) fn kick(&self, force: bool) {
        if !self.gate(Instant::now(), force) {
            return;
        }
        let claude_bin = self.claude_bin.clone();
        let manager_available = self.manager_available;
        tokio::task::spawn_blocking(move || {
            let booted = boot(
                manager_available,
                || super::claude_service::ensure(&claude_bin),
                || spawn_direct(&claude_bin),
            );
            if booted == Booted::Managed {
                tracing::debug!("managed claude daemon service ensured running");
            }
        });
    }
}

// Augmented PATH: `claude` lives off minimal service PATHs.
fn direct_command(claude_bin: &str) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(claude_bin);
    cmd.args(["daemon", "run"])
        .env("PATH", crate::childenv::child_path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    crate::childenv::ScrubChildEnv::scrub_child_env(&mut cmd);
    #[cfg(unix)]
    cmd.process_group(0);
    cmd
}

/// The child is not awaited: `claude daemon run` is the supervisor itself and
/// stays up for its whole life. Own process group so our signals don't reach
/// it; the detached reaper task prevents zombies. Must run within a
/// Tokio runtime.
fn spawn_direct(claude_bin: &str) {
    tracing::info!("no usable service manager — booting `claude daemon run` as direct child");
    match direct_command(claude_bin).spawn() {
        Ok(mut child) => {
            tokio::spawn(async move {
                match child.wait().await {
                    Ok(s) => tracing::info!(code = ?s.code(), "`claude daemon run` exited"),
                    Err(err) => tracing::warn!(%err, "waiting on `claude daemon run`"),
                }
            });
        }
        Err(err) => tracing::warn!(%err, "failed to spawn `claude daemon run`"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn gate_permits_first_then_backs_off() {
        let k = Kickstarter::with_manager("claude".into(), true);
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
        let k = Kickstarter::with_manager("claude".into(), true);
        let t0 = Instant::now();
        assert!(k.gate(t0, true));
        // Forced again immediately: still permitted...
        assert!(k.gate(t0 + Duration::from_millis(1), true));
        // ...and the forced attempt reset the clock, so an unforced one right
        // after is denied.
        assert!(!k.gate(t0 + Duration::from_millis(2), false));
    }

    #[test]
    fn boot_uses_managed_service_when_manager_available() {
        let spawned = AtomicBool::new(false);
        let booted = boot(true, || Ok(()), || spawned.store(true, Ordering::SeqCst));
        assert_eq!(booted, Booted::Managed);
        assert!(!spawned.load(Ordering::SeqCst), "must not spawn a direct child");
    }

    #[test]
    fn boot_skips_service_manager_entirely_when_unavailable() {
        let ensured = AtomicBool::new(false);
        let spawned = AtomicBool::new(false);
        let booted = boot(
            false,
            || {
                ensured.store(true, Ordering::SeqCst);
                Ok(())
            },
            || spawned.store(true, Ordering::SeqCst),
        );
        assert_eq!(booted, Booted::Direct);
        assert!(!ensured.load(Ordering::SeqCst), "must not touch the service manager");
        assert!(spawned.load(Ordering::SeqCst));
    }

    #[test]
    fn boot_falls_back_to_direct_spawn_when_ensure_fails() {
        let spawned = AtomicBool::new(false);
        let booted = boot(
            true,
            || anyhow::bail!("Failed to connect to bus: No medium found"),
            || spawned.store(true, Ordering::SeqCst),
        );
        assert_eq!(booted, Booted::DirectFallback);
        assert!(spawned.load(Ordering::SeqCst), "ensure failure must trigger the direct spawn");
    }

    #[test]
    fn direct_command_runs_claude_daemon_run_detached_with_augmented_path() {
        let cmd = direct_command("/opt/homebrew/bin/claude");
        let std = cmd.as_std();
        assert_eq!(std.get_program(), "/opt/homebrew/bin/claude");
        let args: Vec<_> = std.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(args, ["daemon", "run"]);
        let path = std
            .get_envs()
            .find(|(k, _)| *k == "PATH")
            .and_then(|(_, v)| v)
            .expect("explicit PATH for the child")
            .to_string_lossy()
            .into_owned();
        for want in ["/usr/local/bin", "/usr/bin", "/bin"] {
            assert!(path.split(':').any(|d| d == want), "augmented PATH must contain {want}");
        }
    }

    /// The socket-wait path (`kick(true)` from `ensure_socket`) must actually
    /// exec the fallback child in a no-service-manager environment: kick a
    /// stub `claude` that records its argv to a marker file, then wait for it.
    #[tokio::test]
    async fn kick_spawns_direct_child_without_service_manager() {
        let dir = tempfile::tempdir().expect("tempdir");
        let marker = dir.path().join("argv");
        let stub = dir.path().join("claude");
        std::fs::write(&stub, format!("#!/bin/sh\necho \"$@\" > {}\n", marker.display()))
            .expect("write stub");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755))
                .expect("chmod stub");
        }

        let k = Kickstarter::with_manager(stub.to_string_lossy().into_owned(), false);
        k.kick(true);

        for _ in 0..100 {
            if marker.exists() {
                let argv = std::fs::read_to_string(&marker).expect("read marker");
                assert_eq!(argv.trim(), "daemon run");
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("fallback child was never spawned (no marker file)");
    }
}
