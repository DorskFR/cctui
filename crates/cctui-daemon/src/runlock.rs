//! Singleton run-lock: at most one `cctui-daemon run` serves per machine.
//!
//! `flock(LOCK_EX | LOCK_NB)` on a per-user lock file; the kernel releases it
//! on process exit so a crash leaves no stale lock. `FD_CLOEXEC` is cleared so
//! the fd survives `selfupdate::reexec()` (execve, same pid) — else the
//! re-exec'd image would deadlock against its own lock.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::fd::AsFd;
use std::path::PathBuf;

use anyhow::{Result, bail};
use rustix::fs::{FlockOperation, flock};
use rustix::io::{FdFlags, fcntl_getfd, fcntl_setfd};

const FILE_NAME: &str = "daemon.lock";

/// Held for the process lifetime; dropping (or process exit) releases the lock.
#[derive(Debug)]
pub struct RunLock {
    _file: File,
}

/// Lock-file locations, most preferred first. Mirrors
/// `runtime.rs::candidates()` so the lock lands next to `daemon-runtime.json`
/// and behaves the same across worker containers / missing runtime dirs.
fn candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(d) = dirs::runtime_dir() {
        out.push(d.join("cctui").join(FILE_NAME));
    }
    if let Some(d) = dirs::config_dir() {
        out.push(d.join("cctui").join(FILE_NAME));
    }
    let uid = rustix::process::getuid().as_raw();
    out.push(std::env::temp_dir().join(format!("cctui-{uid}")).join(FILE_NAME));
    out
}

/// Acquire the machine-wide run-lock. Errors non-zero when another daemon
/// already holds it, naming the incumbent's pid/version.
pub fn acquire() -> Result<RunLock> {
    acquire_at(&candidates())
}

fn acquire_at(candidates: &[PathBuf]) -> Result<RunLock> {
    let mut last_err = None;
    for path in candidates {
        let Some(dir) = path.parent() else { continue };
        if let Err(err) = std::fs::create_dir_all(dir) {
            tracing::debug!(path = %dir.display(), %err, "run-lock dir candidate unusable");
            last_err = Some(anyhow::Error::from(err));
            continue;
        }
        let file = match OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
        {
            Ok(f) => f,
            Err(err) => {
                tracing::debug!(path = %path.display(), %err, "run-lock file candidate unopenable");
                last_err = Some(anyhow::Error::from(err));
                continue;
            }
        };
        // Survive `execve` so the re-exec'd self-update image keeps the lock.
        let flags = fcntl_getfd(file.as_fd())?;
        fcntl_setfd(file.as_fd(), flags - FdFlags::CLOEXEC)?;

        match flock(file.as_fd(), FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => {
                let mut file = file;
                let _ = file.set_len(0);
                let _ = write!(file, "{}", std::process::id());
                let _ = file.flush();
                return Ok(RunLock { _file: file });
            }
            Err(err) if err == rustix::io::Errno::WOULDBLOCK || err == rustix::io::Errno::AGAIN => {
                bail!(already_running_message());
            }
            Err(err) => {
                tracing::debug!(path = %path.display(), %err, "run-lock flock candidate failed");
                last_err = Some(anyhow::Error::from(std::io::Error::from(err)));
            }
        }
    }
    match last_err {
        Some(err) => {
            Err(err
                .context("could not acquire the cctui-daemon run-lock at any candidate location"))
        }
        None => bail!("could not acquire the cctui-daemon run-lock: no candidate location"),
    }
}

fn already_running_message() -> String {
    match crate::runtime::read() {
        Some(rt) => format!(
            "another cctui-daemon is already running (pid {}, version {}, since {}) — \
             refusing to start a second instance",
            rt.pid, rt.version, rt.started_at
        ),
        None => "another cctui-daemon is already running — refusing to start a second instance"
            .to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lock_path() -> (tempfile::TempDir, Vec<PathBuf>) {
        let tmp = tempfile::tempdir().unwrap();
        let cands = vec![tmp.path().join("cctui").join(FILE_NAME)];
        (tmp, cands)
    }

    #[test]
    fn second_acquire_fails_while_first_held() {
        let (_tmp, cands) = lock_path();
        let first = acquire_at(&cands).expect("first acquire succeeds");
        let err = acquire_at(&cands).expect_err("second acquire must fail");
        assert!(err.to_string().contains("already running"), "got: {err}");
        drop(first);
    }

    #[test]
    fn lock_released_on_drop() {
        let (_tmp, cands) = lock_path();
        let first = acquire_at(&cands).expect("first acquire succeeds");
        drop(first);
        let _again = acquire_at(&cands).expect("re-acquire after release succeeds");
    }

    #[test]
    fn lock_fd_is_not_cloexec() {
        let (_tmp, cands) = lock_path();
        let lock = acquire_at(&cands).expect("acquire succeeds");
        let flags = fcntl_getfd(lock._file.as_fd()).unwrap();
        assert!(!flags.contains(FdFlags::CLOEXEC), "lock fd must survive execve");
    }

    #[test]
    fn pid_written_into_lock_file() {
        let (_tmp, cands) = lock_path();
        let _lock = acquire_at(&cands).expect("acquire succeeds");
        let contents = std::fs::read_to_string(&cands[0]).unwrap();
        assert_eq!(contents.trim(), std::process::id().to_string());
    }
}
