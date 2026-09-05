//! Singleton run-lock: at most one `cctui-daemon run` serves per machine.
//!
//! `flock(LOCK_EX | LOCK_NB)` on a per-user lock file; the kernel releases it
//! on process exit so a crash leaves no stale lock. The fd must stay
//! `FD_CLOEXEC` so `selfupdate::reexec()` (execve, same pid) releases the
//! lock for the new image to re-acquire: flock locks on separate open file
//! descriptions conflict even within one process, so an inherited fd
//! deadlocks the new image against its own lock. The fd number is exported
//! as [`INHERITED_FD_ENV`] across the re-exec so an image that did leak it
//! (fd cleared of CLOEXEC, or a dup) can close exactly that fd and reclaim
//! the lock when the owner pid is its own, without scanning `/proc/self/fd`.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, Ordering};

use anyhow::{Result, bail};
use rustix::fs::{FlockOperation, flock};

const FILE_NAME: &str = "daemon.lock";

/// Carries the run-lock fd number through `selfupdate::reexec()`.
pub const INHERITED_FD_ENV: &str = "CCTUI_RUNLOCK_FD";

static LOCK_FD: AtomicI32 = AtomicI32::new(-1);

/// Held for the process lifetime; dropping (or process exit) releases the lock.
#[derive(Debug)]
pub struct RunLock {
    file: File,
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

/// Lock-file locations, most preferred first. Shares
/// `runtime::state_candidates` so the lock lands next to
/// `daemon-runtime.json` and the two lists can never drift.
fn candidates() -> Vec<PathBuf> {
    crate::runtime::state_candidates(FILE_NAME)
}

/// Acquire the machine-wide run-lock. Errors non-zero when another daemon
/// already holds it, naming the incumbent's pid/version.
pub fn acquire() -> Result<RunLock> {
    let inherited = std::env::var(INHERITED_FD_ENV).ok().and_then(|v| v.parse().ok());
    let lock = acquire_at(&candidates(), inherited)?;
    LOCK_FD.store(lock.file.as_raw_fd(), Ordering::Relaxed);
    Ok(lock)
}

/// Hand the held lock's fd number to the image `cmd` will execve into.
pub fn export_for_reexec(cmd: &mut std::process::Command) {
    let fd = LOCK_FD.load(Ordering::Relaxed);
    if fd >= 0 {
        cmd.env(INHERITED_FD_ENV, fd.to_string());
    }
}

fn acquire_at(candidates: &[PathBuf], inherited: Option<RawFd>) -> Result<RunLock> {
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
            match lock_with_self_reclaim(&file, path, inherited) {
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
                    return Ok(RunLock { file, path: path.clone() });
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

fn lock_with_self_reclaim(
    file: &File,
    path: &Path,
    inherited: Option<RawFd>,
) -> rustix::io::Result<()> {
    match flock(file.as_fd(), FlockOperation::NonBlockingLockExclusive) {
        Err(err)
            if (err == rustix::io::Errno::WOULDBLOCK || err == rustix::io::Errno::AGAIN)
                && !held_here(path)
                && lock_owner_pid(path) == Some(std::process::id())
                && inherited.is_some_and(|fd| close_inherited_fd(fd, file, path)) =>
        {
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

/// Closes `fd` only when it is open on the lock file's inode (the number may
/// have been reused by an unrelated file since the exec). Returns whether it
/// was closed, releasing its flock.
#[allow(unsafe_code)]
fn close_inherited_fd(fd: RawFd, file: &File, path: &Path) -> bool {
    if fd <= 2 || fd == file.as_raw_fd() {
        return false;
    }
    let Ok(target) = rustix::fs::stat(path) else { return false };
    let borrowed = unsafe { std::os::fd::BorrowedFd::borrow_raw(fd) };
    let Ok(st) = rustix::fs::fstat(borrowed) else { return false };
    if st.st_dev != target.st_dev || st.st_ino != target.st_ino {
        return false;
    }
    drop(unsafe { OwnedFd::from_raw_fd(fd) });
    true
}

fn already_running_message() -> String {
    let mut msg = match crate::runtime::read() {
        Some(rt) => format!(
            "another cctui-daemon is already running (pid {}, version {}, since {}) — \
             refusing to start a second instance",
            rt.pid, rt.version, rt.started_at
        ),
        None => "another cctui-daemon is already running — refusing to start a second instance"
            .to_owned(),
    };
    if crate::service::is_active() {
        msg.push_str(
            "\nthe managed cctui-daemon service holds the lock — use \
             `cctui-daemon service status` / `cctui-daemon service restart` \
             instead of `cctui-daemon run`",
        );
    }
    msg
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
        let first = acquire_at(&cands, None).expect("first acquire succeeds");
        let err = acquire_at(&cands, None).expect_err("second acquire must fail");
        assert!(err.to_string().contains("already running"), "got: {err}");
        drop(first);
    }

    #[test]
    fn lock_released_on_drop() {
        let (_tmp, cands) = lock_path();
        let first = acquire_at(&cands, None).expect("first acquire succeeds");
        drop(first);
        let _again = acquire_at(&cands, None).expect("re-acquire after release succeeds");
    }

    #[test]
    fn lock_fd_is_cloexec() {
        use rustix::io::{FdFlags, fcntl_getfd};
        let (_tmp, cands) = lock_path();
        let lock = acquire_at(&cands, None).expect("acquire succeeds");
        let flags = fcntl_getfd(lock.file.as_fd()).unwrap();
        assert!(flags.contains(FdFlags::CLOEXEC), "lock fd must not survive execve");
    }

    #[test]
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
        let lock = acquire_at(&cands, None).expect("dead-owner lock must be rotated");
        assert!(fd_matches_path(&lock.file, &cands[0]));
        drop(orphan);
    }

    #[test]
    fn self_owned_inherited_lock_is_reclaimed() {
        use std::os::fd::IntoRawFd;
        let (_tmp, cands) = lock_path();
        let first = acquire_at(&cands, None).expect("first acquire succeeds");
        let leaked = first.file.try_clone().unwrap().into_raw_fd();
        drop(first);
        let err = acquire_at(&cands, None).expect_err("unknown leaked fd is not reclaimed");
        assert!(err.to_string().contains("already running"), "got: {err}");
        let again =
            acquire_at(&cands, Some(leaked)).expect("self-owned inherited lock must be reclaimed");
        drop(again);
    }

    #[test]
    fn inherited_fd_on_another_inode_is_left_alone() {
        let (_tmp, cands) = lock_path();
        let other = tempfile::tempfile().unwrap();
        let lock = acquire_at(&cands, None).expect("acquire succeeds");
        assert!(!close_inherited_fd(other.as_raw_fd(), &lock.file, &cands[0]));
        assert!(rustix::fs::fstat(other.as_fd()).is_ok(), "unrelated fd must stay open");
    }

    #[test]
    fn pid_written_into_lock_file() {
        let (_tmp, cands) = lock_path();
        let _lock = acquire_at(&cands, None).expect("acquire succeeds");
        let contents = std::fs::read_to_string(&cands[0]).unwrap();
        assert_eq!(contents.trim(), std::process::id().to_string());
    }
}
