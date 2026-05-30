//! Runtime state file written by the running daemon (`run`).
//!
//! Lets a *separate* CLI invocation (`status` / `service status`) report the
//! version of the **running service** — not just the version compiled into
//! whatever binary happened to be invoked.
//!
//! Without this, after a binary swap it's ambiguous whether the long-lived
//! service is still on the previous build (e.g. not yet restarted). The file
//! is rewritten on every `run` startup — including the self-update re-exec —
//! so it always reflects the process currently serving.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Runtime {
    /// Version the running process was built from (`CARGO_PKG_VERSION`).
    pub version: String,
    /// PID of the running daemon process.
    pub pid: u32,
    /// RFC3339 timestamp of when the process recorded this state.
    pub started_at: String,
}

/// Prefer the per-user runtime dir (cleared on logout/reboot, so it never goes
/// stale across a crash) and fall back to the config dir on platforms without
/// one (e.g. some macOS setups).
pub fn path() -> PathBuf {
    dirs::runtime_dir()
        .or_else(dirs::config_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("cctui")
        .join("daemon-runtime.json")
}

/// Record the current process as the running daemon. Best-effort: a failure to
/// write is logged but never blocks startup.
pub fn record() {
    let rt = Runtime {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        pid: std::process::id(),
        started_at: chrono::Utc::now().to_rfc3339(),
    };
    let p = path();
    if let Some(dir) = p.parent()
        && let Err(err) = std::fs::create_dir_all(dir)
    {
        tracing::warn!(path = %dir.display(), %err, "failed to create runtime dir");
        return;
    }
    match serde_json::to_string_pretty(&rt) {
        Ok(json) => {
            if let Err(err) = std::fs::write(&p, json) {
                tracing::warn!(path = %p.display(), %err, "failed to write runtime state");
            }
        }
        Err(err) => tracing::warn!(%err, "failed to serialize runtime state"),
    }
}

/// Read the recorded runtime state, if any. `None` when no daemon has run on
/// this machine since the file location was last cleared.
#[must_use]
pub fn read() -> Option<Runtime> {
    let data = std::fs::read_to_string(path()).ok()?;
    serde_json::from_str(&data).ok()
}

/// Whether the recorded PID is still a live process. Used to tell a current
/// `version` from one left behind by a daemon that has since exited.
#[must_use]
pub fn pid_alive(pid: u32) -> bool {
    i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
        .is_some_and(|p| rustix::process::test_kill_process(p).is_ok())
}
