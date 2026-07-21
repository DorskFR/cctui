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

const FILE_NAME: &str = "daemon-runtime.json";

/// Most preferred first. Worker containers can lack a runtime dir and have a
/// root-owned `~/.config` where `mkdir` is EACCES (CCT-629); readers must probe
/// this same list so `status` finds whatever the daemon could write.
pub(crate) fn state_candidates(file_name: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(d) = dirs::runtime_dir() {
        out.push(d.join("cctui").join(file_name));
    }
    if let Some(d) = dirs::config_dir() {
        out.push(d.join("cctui").join(file_name));
    }
    let uid = rustix::process::getuid().as_raw();
    out.push(std::env::temp_dir().join(format!("cctui-{uid}")).join(file_name));
    out
}

fn candidates() -> Vec<PathBuf> {
    state_candidates(FILE_NAME)
}

pub(crate) fn record_at(candidates: &[PathBuf], json: &str) -> Option<PathBuf> {
    for p in candidates {
        let Some(dir) = p.parent() else { continue };
        if let Err(err) = std::fs::create_dir_all(dir) {
            tracing::debug!(path = %dir.display(), %err, "runtime dir candidate unusable");
            continue;
        }
        match std::fs::write(p, json) {
            Ok(()) => return Some(p.clone()),
            Err(err) => {
                tracing::debug!(path = %p.display(), %err, "runtime state candidate unwritable");
            }
        }
    }
    None
}

fn read_at(candidates: &[PathBuf]) -> Option<Runtime> {
    candidates.iter().find_map(|p| serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok())
}

/// Record the current process as the running daemon. Best-effort: a failure to
/// write is logged but never blocks startup.
pub fn record() {
    let rt = Runtime {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        pid: std::process::id(),
        started_at: chrono::Utc::now().to_rfc3339(),
    };
    match serde_json::to_string_pretty(&rt) {
        Ok(json) => {
            let cands = candidates();
            if record_at(&cands, &json).is_none() {
                tracing::warn!(
                    tried = %cands.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", "),
                    "failed to write runtime state to any candidate location"
                );
            }
        }
        Err(err) => tracing::warn!(%err, "failed to serialize runtime state"),
    }
}

/// Read the recorded runtime state, if any. `None` when no daemon has run on
/// this machine since the file location was last cleared.
#[must_use]
pub fn read() -> Option<Runtime> {
    read_at(&candidates())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_json() -> String {
        serde_json::to_string_pretty(&Runtime {
            version: "0.0.0-test".into(),
            pid: std::process::id(),
            started_at: chrono::Utc::now().to_rfc3339(),
        })
        .unwrap()
    }

    #[test]
    fn record_falls_back_past_uncreatable_dir_and_read_finds_it() {
        let tmp = tempfile::tempdir().unwrap();
        let denied = tmp.path().join("denied");
        std::fs::create_dir(&denied).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&denied, std::fs::Permissions::from_mode(0o555)).unwrap();
        }
        let cands =
            vec![denied.join("cctui").join(FILE_NAME), tmp.path().join("writable").join(FILE_NAME)];

        let written = record_at(&cands, &sample_json()).expect("a fallback candidate must work");
        assert_eq!(written, cands[1]);
        let rt = read_at(&cands).expect("read probes the same candidate list");
        assert_eq!(rt.version, "0.0.0-test");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&denied, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn record_reports_none_when_every_candidate_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let denied = tmp.path().join("denied");
        std::fs::create_dir(&denied).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&denied, std::fs::Permissions::from_mode(0o555)).unwrap();
        }
        let cands = vec![denied.join("cctui").join(FILE_NAME)];
        #[cfg(unix)]
        assert!(record_at(&cands, &sample_json()).is_none());
        assert!(read_at(&cands).is_none());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&denied, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn candidates_always_include_a_temp_fallback() {
        let cands = candidates();
        assert!(!cands.is_empty());
        assert!(cands.last().unwrap().starts_with(std::env::temp_dir()));
    }
}
