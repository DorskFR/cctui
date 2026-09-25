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

use std::path::{Path, PathBuf};

use anyhow::Context as _;

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
/// root-owned `~/.config` where `mkdir` is EACCES; readers must probe
/// this same list so `status` finds whatever the daemon could write.
///
/// No `$TMPDIR` fallback on macOS: launchd and a login shell see different
/// `/var/folders/…` paths, so a tmp-based run-lock lets a manual `run` and
/// the agent double-run (or contend on nothing). `~/Library/Application
/// Support` is deterministic per user in both contexts.
pub(crate) fn state_candidates(file_name: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(d) = dirs::runtime_dir() {
        out.push(d.join("cctui").join(file_name));
    }
    if let Some(d) = dirs::config_dir() {
        out.push(d.join("cctui").join(file_name));
    }
    if !cfg!(target_os = "macos") {
        let uid = rustix::process::getuid().as_raw();
        out.push(std::env::temp_dir().join(format!("cctui-{uid}")).join(file_name));
    }
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

/// Default path for a local IPC socket: `$XDG_RUNTIME_DIR/<name>`, otherwise
/// a per-user private dir (never a shared `/tmp` path another user could squat).
pub(crate) fn socket_path(file_name: &str) -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|v| !v.is_empty())
        .map_or_else(|| private_socket_dir().join(file_name), |d| PathBuf::from(d).join(file_name))
}

fn private_socket_dir() -> PathBuf {
    if cfg!(target_os = "macos")
        && let Some(d) = dirs::config_dir()
    {
        return d.join("cctui");
    }
    let uid = rustix::process::getuid().as_raw();
    std::env::temp_dir().join(format!("cctui-{uid}"))
}

/// Create `dir` if missing and require it to be a real directory owned by us,
/// tightened to 0700.
pub(crate) fn ensure_private_dir(dir: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    if let Err(err) = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)
        && err.kind() != std::io::ErrorKind::AlreadyExists
    {
        return Err(err).with_context(|| format!("create {}", dir.display()));
    }
    let meta = std::fs::symlink_metadata(dir).with_context(|| format!("stat {}", dir.display()))?;
    if !meta.is_dir() {
        anyhow::bail!("{} is not a directory", dir.display());
    }
    let uid = rustix::process::getuid().as_raw();
    if meta.uid() != uid {
        anyhow::bail!("{} is owned by uid {}, not {uid}", dir.display(), meta.uid());
    }
    if meta.mode() & 0o077 != 0 {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Bind a 0600 Unix socket at `path`. The private fallback dir is created and
/// ownership-checked; an existing socket file owned by another uid is an error
/// rather than something to silently fail to replace.
pub(crate) fn bind_private_socket(path: &Path) -> anyhow::Result<tokio::net::UnixListener> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let parent = path.parent().context("socket path has no parent")?;
    if parent == private_socket_dir() {
        ensure_private_dir(parent)?;
    } else {
        std::fs::create_dir_all(parent)?;
    }
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        let uid = rustix::process::getuid().as_raw();
        if meta.uid() != uid {
            anyhow::bail!("socket {} is owned by uid {}, not {uid}", path.display(), meta.uid());
        }
        std::fs::remove_file(path).with_context(|| format!("remove stale {}", path.display()))?;
    }
    let listener = tokio::net::UnixListener::bind(path)
        .with_context(|| format!("bind {}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_private_dir_creates_a_0700_dir_and_tightens_loose_ones() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("a/b");
        ensure_private_dir(&dir).unwrap();
        let mode = |d: &Path| std::fs::metadata(d).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).unwrap();
        ensure_private_dir(&dir).unwrap();
        assert_eq!(mode(&dir), 0o700);
    }

    #[test]
    fn ensure_private_dir_rejects_symlinks_and_files() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("f");
        std::fs::write(&file, b"").unwrap();
        assert!(ensure_private_dir(&file).is_err());
        let link = tmp.path().join("l");
        std::os::unix::fs::symlink(tmp.path(), &link).unwrap();
        assert!(ensure_private_dir(&link).is_err());
    }

    #[test]
    fn socket_fallback_is_a_private_per_user_dir() {
        let dir = private_socket_dir();
        let uid = rustix::process::getuid().as_raw();
        if cfg!(target_os = "macos") {
            assert!(dir.ends_with("cctui"));
        } else {
            assert_eq!(dir, std::env::temp_dir().join(format!("cctui-{uid}")));
        }
        assert_ne!(dir.join("x.sock"), Path::new("/tmp/x.sock"));
    }

    #[tokio::test]
    async fn bind_private_socket_replaces_own_stale_socket_with_0600() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("s.sock");
        drop(bind_private_socket(&path).unwrap());
        let _l = bind_private_socket(&path).expect("own stale socket is replaced");
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }

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

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn candidates_always_include_a_temp_fallback() {
        let cands = candidates();
        assert!(!cands.is_empty());
        assert!(cands.last().unwrap().starts_with(std::env::temp_dir()));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn candidates_never_use_tmpdir_on_macos() {
        let cands = candidates();
        assert!(!cands.is_empty());
        assert!(cands.iter().all(|p| !p.starts_with(std::env::temp_dir())));
    }
}
