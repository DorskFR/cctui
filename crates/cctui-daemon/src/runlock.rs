//! Singleton run-lock: at most one `cctui-daemon run` serves per machine.
//!
//! `flock(LOCK_EX | LOCK_NB)` on a per-user lock file; the kernel releases it
//! on process exit so a crash leaves no stale lock. The fd must stay
//! `FD_CLOEXEC` so `selfupdate::reexec()` (execve, same pid) releases the
//! lock for the new image to re-acquire: flock locks on separate open file
//! descriptions conflict even within one process, so an inherited fd
//! deadlocks the new image against its own lock. Fds leaked by pre-CLOEXEC
//! binaries are reclaimed when the lock owner pid is our own.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use rustix::fs::{FlockOperation, flock};

const FILE_NAME: &str = "daemon.lock";

/// Held for the process lifetime; dropping (or process exit) releases the lock.
#[derive(Debug)]
pub struct RunLock {
    _file: File,
    path: PathBuf,
}

/// Distinguishes an execve-inherited lock (no live holder in this image —
/// reclaimable) from one a live `RunLock` in this process still owns.
static HELD_IN_THIS_IMAGE: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

fn held_here(path: &Path) -> bool {
    HELD_IN_THIS_IMAGE.lock().is_ok_and(|held| held.iter().any(|p| p == path))
}

impl Drop for RunLock {
    fn drop(&mut self) {
        if let Ok(mut held) = HELD_IN_THIS_IMAGE.lock() {
            held.retain(|p| p != &self.path);
        }
    }
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
    'candidate: for path in candidates {
        let Some(dir) = path.parent() else { continue };
        if let Err(err) = std::fs::create_dir_all(dir) {
            tracing::debug!(path = %dir.display(), %err, "run-lock dir candidate unusable");
            last_err = Some(anyhow::Error::from(err));
            continue;
        }
        for _ in 0..3 {
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
                    continue 'candidate;
                }
            };
            match lock_with_self_reclaim(&file, path) {
                Ok(()) => {
                    if !fd_matches_path(&file, path) {
                        continue;
                    }
                    let mut file = file;
                    let _ = file.set_len(0);
                    let _ = write!(file, "{}", std::process::id());
                    let _ = file.flush();
                    if let Ok(mut held) = HELD_IN_THIS_IMAGE.lock() {
                        held.push(path.clone());
                    }
                    return Ok(RunLock { _file: file, path: path.clone() });
                }
                Err(err)
                    if err == rustix::io::Errno::WOULDBLOCK || err == rustix::io::Errno::AGAIN =>
                {
                    if !held_here(path) && lock_owner_dead(path) {
                        tracing::warn!(
                            path = %path.display(),
                            "run-lock owner is dead but the lock is held (fds orphaned in its \
                             children) — rotating the lock file"
                        );
                        let _ = std::fs::remove_file(path);
                        continue;
                    }
                    bail!(already_running_message());
                }
                Err(err) => {
                    tracing::debug!(path = %path.display(), %err, "run-lock flock candidate failed");
                    last_err = Some(anyhow::Error::from(std::io::Error::from(err)));
                    continue 'candidate;
                }
            }
        }
        bail!(already_running_message());
    }
    match last_err {
        Some(err) => {
            Err(err
                .context("could not acquire the cctui-daemon run-lock at any candidate location"))
        }
        None => bail!("could not acquire the cctui-daemon run-lock: no candidate location"),
    }
}

fn lock_with_self_reclaim(file: &File, path: &Path) -> rustix::io::Result<()> {
    match flock(file.as_fd(), FlockOperation::NonBlockingLockExclusive) {
        Err(err)
            if (err == rustix::io::Errno::WOULDBLOCK || err == rustix::io::Errno::AGAIN)
                && !held_here(path)
                && lock_owner_pid(path) == Some(std::process::id()) =>
        {
            close_inherited_fds(file, path);
            flock(file.as_fd(), FlockOperation::NonBlockingLockExclusive)
        }
        other => other,
    }
}

fn lock_owner_pid(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn lock_owner_dead(path: &Path) -> bool {
    lock_owner_pid(path).is_some_and(|pid| !crate::runtime::pid_alive(pid))
}

/// After a rotation race, the locked fd may point at an inode another daemon
/// just unlinked; only a lock on the inode currently at `path` counts.
fn fd_matches_path(file: &File, path: &Path) -> bool {
    match (rustix::fs::fstat(file.as_fd()), rustix::fs::stat(path)) {
        (Ok(a), Ok(b)) => a.st_dev == b.st_dev && a.st_ino == b.st_ino,
        _ => false,
    }
}

/// Any other fd open on the lock file's inode was leaked across execve by a
/// pre-CLOEXEC image of this same process; closing it releases its flock.
#[allow(unsafe_code)]
fn close_inherited_fds(file: &File, path: &Path) {
    let Ok(target) = rustix::fs::stat(path) else { return };
    #[cfg(target_os = "linux")]
    let fd_dir = "/proc/self/fd";
    #[cfg(not(target_os = "linux"))]
    let fd_dir = "/dev/fd";
    let Ok(entries) = std::fs::read_dir(fd_dir) else { return };
    let fds: Vec<RawFd> = entries
        .filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok())
        .filter(|&fd| fd > 2 && fd != file.as_raw_fd())
        .collect();
    for fd in fds {
        let borrowed = unsafe { std::os::fd::BorrowedFd::borrow_raw(fd) };
        if let Ok(st) = rustix::fs::fstat(borrowed)
            && st.st_dev == target.st_dev
            && st.st_ino == target.st_ino
        {
            drop(unsafe { OwnedFd::from_raw_fd(fd) });
        }
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
    #[allow(clippy::used_underscore_binding)]
    fn lock_fd_is_cloexec() {
        use rustix::io::{FdFlags, fcntl_getfd};
        let (_tmp, cands) = lock_path();
        let lock = acquire_at(&cands).expect("acquire succeeds");
        let flags = fcntl_getfd(lock._file.as_fd()).unwrap();
        assert!(flags.contains(FdFlags::CLOEXEC), "lock fd must not survive execve");
    }

    #[test]
    #[allow(clippy::used_underscore_binding)]
    fn dead_owner_lock_is_rotated() {
        let (_tmp, cands) = lock_path();
        std::fs::create_dir_all(cands[0].parent().unwrap()).unwrap();
        let orphan = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&cands[0])
            .unwrap();
        flock(orphan.as_fd(), FlockOperation::NonBlockingLockExclusive).unwrap();
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let dead_pid = child.id();
        child.wait().unwrap();
        std::fs::write(&cands[0], dead_pid.to_string()).unwrap();
        let lock = acquire_at(&cands).expect("dead-owner lock must be rotated");
        assert!(fd_matches_path(&lock._file, &cands[0]));
        drop(orphan);
    }

    #[test]
    #[allow(clippy::used_underscore_binding)]
    fn self_owned_inherited_lock_is_reclaimed() {
        use std::os::fd::IntoRawFd;
        let (_tmp, cands) = lock_path();
        let first = acquire_at(&cands).expect("first acquire succeeds");
        let _leaked = first._file.try_clone().unwrap().into_raw_fd();
        drop(first);
        let again = acquire_at(&cands).expect("self-owned lock must be reclaimed");
        drop(again);
    }

    #[test]
    fn pid_written_into_lock_file() {
        let (_tmp, cands) = lock_path();
        let _lock = acquire_at(&cands).expect("acquire succeeds");
        let contents = std::fs::read_to_string(&cands[0]).unwrap();
        assert_eq!(contents.trim(), std::process::id().to_string());
    }
}
