//! The exec notification loop. Every notification gets an answer: allow,
//! or `EPERM` on a policy denial, an unreadable target, a crash or a timeout.

use std::fs::{File, Permissions};
use std::io::{self, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use super::argv::{self, ExecRequest, Memory};
use super::decide::{self, Outcome};
use super::sys::{self, Notif, Sizes};
use super::why::{Denial, WhyRing};
use crate::engine::WorkflowEngine;

const STDERR_WRITE_TIMEOUT: Duration = Duration::from_millis(250);
const CAP_SYS_PTRACE: u32 = 19;
const AT_EMPTY_PATH: u64 = 0x1000;
const AT_FDCWD: u32 = 0xFFFF_FF9C;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecKind {
    Execve,
    Execveat,
}

/// `None` (denied) for anything but a native exec, including the x32 numbers
/// the filter also routes here.
#[must_use]
pub const fn classify(arch: u32, nr: i32) -> Option<ExecKind> {
    let (native, execve, execveat) = if cfg!(target_arch = "x86_64") {
        (0xC000_003E, 59, 322)
    } else if cfg!(target_arch = "aarch64") {
        (0xC000_00B7, 221, 281)
    } else {
        return None;
    };
    if arch != native {
        return None;
    }
    if nr == execve {
        Some(ExecKind::Execve)
    } else if nr == execveat {
        Some(ExecKind::Execveat)
    } else {
        None
    }
}

#[must_use]
pub fn has_cap(status: &str, bit: u32) -> bool {
    status
        .lines()
        .find_map(|l| l.strip_prefix("CapEff:"))
        .and_then(|v| u64::from_str_radix(v.trim(), 16).ok())
        .is_some_and(|caps| caps & (1 << bit) != 0)
}

#[must_use]
pub fn resolve_at(
    path: &str,
    dirfd: u32,
    flags: u64,
    link: impl Fn(u32) -> Option<String>,
) -> String {
    if path.is_empty() && flags & AT_EMPTY_PATH != 0 {
        return link(dirfd).unwrap_or_else(|| format!("fd:{dirfd}"));
    }
    if path.starts_with('/') || dirfd == AT_FDCWD {
        return path.to_string();
    }
    link(dirfd)
        .map_or_else(|| path.to_string(), |dir| format!("{}/{path}", dir.trim_end_matches('/')))
}

pub fn read_request(
    mem: &impl Memory,
    pid: u32,
    kind: ExecKind,
    regs: &[u64; 6],
) -> io::Result<ExecRequest> {
    let (path_ptr, argv_ptr) = match kind {
        ExecKind::Execve => (regs[0], regs[1]),
        ExecKind::Execveat => (regs[1], regs[2]),
    };
    let path = argv::read_cstring(mem, path_ptr, argv::MAX_PATH)?;
    let path = String::from_utf8_lossy(&path).into_owned();
    let argv = argv::read_argv(mem, argv_ptr)?;
    let path = match kind {
        ExecKind::Execve => path,
        ExecKind::Execveat => resolve_at(&path, regs[0] as u32, regs[4], |fd| {
            let link = std::fs::read_link(format!("/proc/{pid}/fd/{fd}")).ok()?;
            Some(link.to_string_lossy().into_owned())
        }),
    };
    Ok(ExecRequest { path, argv })
}

enum Inspected {
    /// The target died or its syscall was interrupted; nothing to answer.
    Gone,
    Unreadable(String),
    Checked {
        outcome: Outcome,
        request: ExecRequest,
    },
}

pub fn bind(path: &Path) -> io::Result<UnixListener> {
    if let Err(e) = std::fs::remove_file(path)
        && e.kind() != io::ErrorKind::NotFound
    {
        return Err(e);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, Permissions::from_mode(0o600))?;
    Ok(listener)
}

/// Bind `path` now, then accept the supervisor's listener and serve it on a
/// background thread.
pub fn spawn(
    path: &Path,
    engine: Arc<WorkflowEngine>,
    why: Arc<WhyRing>,
    timeout: Duration,
) -> io::Result<()> {
    let socket = bind(path)?;
    let guard = ExecGuard { engine, why, timeout };
    let path = path.to_path_buf();
    std::thread::Builder::new()
        .name("guard-exec".into())
        .spawn(move || guard.serve(&socket, &path))?;
    Ok(())
}

struct ExecGuard {
    engine: Arc<WorkflowEngine>,
    why: Arc<WhyRing>,
    timeout: Duration,
}

fn preflight() -> Result<Sizes, String> {
    let sizes =
        sys::notif_sizes().map_err(|e| format!("seccomp user notification unsupported: {e}"))?;
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    if !has_cap(&status, CAP_SYS_PTRACE) {
        return Err("cctui-guard lacks CAP_SYS_PTRACE, so it cannot read worker argv".to_string());
    }
    Ok(sizes)
}

fn reply(mut stream: &UnixStream, text: &str) {
    let _ = stream.write_all(text.as_bytes());
}

/// Take the listener from the first connection by root or our own uid.
fn accept(socket: &UnixListener) -> Option<(OwnedFd, Sizes)> {
    let me = sys::effective_uid();
    for conn in socket.incoming() {
        let stream = match conn {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("exec hand-off accept failed: {e}");
                continue;
            }
        };
        let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
        match sys::peer_uid(&stream) {
            Ok(uid) if uid == 0 || uid == me => {}
            other => {
                tracing::warn!("exec hand-off from unexpected peer {other:?}; ignored");
                reply(&stream, "refused: unexpected peer\n");
                continue;
            }
        }
        let listener = match sys::recv_fd(&stream) {
            Ok(fd) => fd,
            Err(e) => {
                reply(&stream, &format!("refused: no listener fd ({e})\n"));
                return None;
            }
        };
        return match preflight() {
            Ok(sizes) => {
                reply(&stream, "ok\n");
                Some((listener, sizes))
            }
            Err(reason) => {
                tracing::error!("exec hand-off refused: {reason}");
                reply(&stream, &format!("refused: {reason}\n"));
                None
            }
        };
    }
    None
}

impl ExecGuard {
    fn serve(&self, socket: &UnixListener, path: &Path) {
        let accepted = accept(socket);
        let _ = std::fs::remove_file(path);
        match accepted {
            Some((listener, sizes)) => {
                tracing::info!("exec interception armed");
                self.run(listener, sizes);
            }
            None => {
                tracing::error!("exec interception not armed; the supervisor will refuse to start");
            }
        }
    }

    fn run(&self, listener: OwnedFd, sizes: Sizes) {
        let listener = Arc::new(listener);
        loop {
            match sys::recv(listener.as_fd(), sizes) {
                Ok(notif) => self.dispatch(&listener, sizes, notif),
                Err(e) if matches!(e.raw_os_error(), Some(libc::EINTR | libc::ENOENT)) => {}
                Err(e) => {
                    tracing::error!("exec notify loop stopped ({e}); every further exec fails");
                    return;
                }
            }
        }
    }

    fn dispatch(&self, listener: &Arc<OwnedFd>, sizes: Sizes, notif: Notif) {
        let task = Task {
            listener: Arc::clone(listener),
            sizes,
            notif,
            engine: Arc::clone(&self.engine),
            why: Arc::clone(&self.why),
            timeout: self.timeout,
        };
        let spawned = std::thread::Builder::new()
            .name("guard-exec-notif".into())
            .spawn(move || task.handle());
        if spawned.is_err() {
            let _ = sys::respond(listener.as_fd(), sizes, notif.id, false);
        }
    }
}

struct Task {
    listener: Arc<OwnedFd>,
    sizes: Sizes,
    notif: Notif,
    engine: Arc<WorkflowEngine>,
    why: Arc<WhyRing>,
    timeout: Duration,
}

impl Task {
    fn handle(self) {
        let listener = Arc::clone(&self.listener);
        let engine = Arc::clone(&self.engine);
        let notif = self.notif;
        let checked = decide::within(self.timeout, move || inspect(&listener, notif, &engine));
        let (outcome, request) = match checked {
            Ok(Inspected::Gone) => return,
            Ok(Inspected::Checked { outcome, request }) => (outcome, Some(request)),
            Ok(Inspected::Unreadable(reason)) => (Outcome::Deny(reason), None),
            Err(failure) => (Outcome::Deny(failure.reason().to_string()), None),
        };
        let allow = match outcome {
            Outcome::Allow => true,
            Outcome::Deny(reason) => {
                self.deny(request.as_ref(), &reason);
                false
            }
        };
        if let Err(e) = sys::respond(self.listener.as_fd(), self.sizes, notif.id, allow) {
            tracing::debug!("exec notification {} not answered: {e}", notif.id);
        }
    }

    /// Record the denial and tell the target why, before it sees `EPERM`.
    fn deny(&self, request: Option<&ExecRequest>, reason: &str) {
        let command = request.map(ExecRequest::display);
        if request.is_none() {
            self.engine.decision_log().exec(None, "", &[], false, reason);
        }
        tracing::info!("DENY exec pid={} {command:?}: {reason}", self.notif.pid);
        self.why.push(Denial {
            pid: self.notif.pid,
            command: command.clone().unwrap_or_else(|| "exec".to_string()),
            reason: reason.to_string(),
        });
        self.write_stderr(argv::denial_message(command.as_deref(), reason));
    }

    /// Write on the target's own fd 2 (duplicated, never re-opened, so the
    /// guard's privileges never reach a file the target could only read). The
    /// write is abandoned after a short wait so a full pipe cannot stall the
    /// answer.
    fn write_stderr(&self, message: String) {
        let Ok(pidfd) = sys::pidfd_open(self.notif.pid) else {
            return;
        };
        if !sys::id_valid(self.listener.as_fd(), self.notif.id) {
            return;
        }
        let Ok(stderr) = sys::pidfd_getfd(pidfd.as_fd(), 2) else {
            return;
        };
        if !sys::is_writable(stderr.as_fd()) {
            return;
        }
        let (tx, rx) = mpsc::sync_channel(1);
        let spawned =
            std::thread::Builder::new().name("guard-exec-stderr".into()).spawn(move || {
                let _ = File::from(stderr).write_all(message.as_bytes());
                let _ = tx.send(());
            });
        if spawned.is_ok() {
            let _ = rx.recv_timeout(STDERR_WRITE_TIMEOUT);
        }
    }
}

/// Read the exec out of the target and judge it. The notification id is
/// re-validated after opening the target's memory (so the pid still names
/// the target) and after reading it (so the syscall is still the one pending).
fn inspect(listener: &OwnedFd, notif: Notif, engine: &WorkflowEngine) -> Inspected {
    let Some(kind) = classify(notif.arch, notif.nr) else {
        return Inspected::Unreadable(format!(
            "unsupported exec syscall {} (arch {:#x})",
            notif.nr, notif.arch
        ));
    };
    let valid = || sys::id_valid(listener.as_fd(), notif.id);
    let mem = match File::open(format!("/proc/{}/mem", notif.pid)) {
        Ok(f) => f,
        Err(_) if !valid() => return Inspected::Gone,
        Err(e) => return Inspected::Unreadable(format!("cannot read the process's memory: {e}")),
    };
    if !valid() {
        return Inspected::Gone;
    }
    let request = match read_request(&mem, notif.pid, kind, &notif.args) {
        Ok(r) => r,
        Err(_) if !valid() => return Inspected::Gone,
        Err(e) => return Inspected::Unreadable(format!("cannot read the exec arguments: {e}")),
    };
    if !valid() {
        return Inspected::Gone;
    }
    let verdict = engine.check_exec(&request.path, &request.argv, &request.match_commands());
    Inspected::Checked { outcome: verdict.into(), request }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_native_exec_syscalls_only() {
        if cfg!(target_arch = "x86_64") {
            assert_eq!(classify(0xC000_003E, 59), Some(ExecKind::Execve));
            assert_eq!(classify(0xC000_003E, 322), Some(ExecKind::Execveat));
            assert_eq!(classify(0xC000_003E, 0x4000_0000 | 520), None);
            assert_eq!(classify(0x4000_0003, 11), None);
            assert_eq!(classify(0xC000_003E, 0), None);
        }
        if cfg!(target_arch = "aarch64") {
            assert_eq!(classify(0xC000_00B7, 221), Some(ExecKind::Execve));
            assert_eq!(classify(0xC000_00B7, 281), Some(ExecKind::Execveat));
            assert_eq!(classify(0x4000_0028, 11), None);
        }
    }

    #[test]
    fn reads_cap_eff() {
        let status = "Name:\tx\nCapPrm:\t0000000000000000\nCapEff:\t00000000a80c25fb\n";
        assert!(has_cap(status, CAP_SYS_PTRACE));
        assert!(has_cap("CapEff:\t0000000000080000\n", CAP_SYS_PTRACE));
        assert!(!has_cap("CapEff:\t00000000a80425fb\n", CAP_SYS_PTRACE));
        assert!(!has_cap("garbage", CAP_SYS_PTRACE));
    }

    #[test]
    fn resolves_execveat_paths() {
        let link = |fd: u32| (fd == 7).then(|| "/opt/tools".to_string());
        assert_eq!(resolve_at("/usr/bin/git", 7, 0, link), "/usr/bin/git");
        assert_eq!(resolve_at("git", AT_FDCWD, 0, link), "git");
        assert_eq!(resolve_at("git", 7, 0, link), "/opt/tools/git");
        assert_eq!(resolve_at("", 7, AT_EMPTY_PATH, link), "/opt/tools");
        assert_eq!(resolve_at("", 9, AT_EMPTY_PATH, link), "fd:9");
    }
}
