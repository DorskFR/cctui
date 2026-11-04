//! Exec interception: a `SECCOMP_RET_USER_NOTIF` filter on `execve`/`execveat`
//! whose listener fd is handed to cctui-guard over a unix socket.
//!
//! Once the listener is closed (guard died) the kernel fails every exec with
//! `ENOSYS`, so the payload tree never execs unsupervised.

use std::io::Read;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};

/// How long the supervisor waits for the guard to accept the listener.
pub const HANDOFF_TIMEOUT: Duration = Duration::from_secs(10);

/// The guard's reply when it took the listener and runs the notify loop.
pub const ACK_OK: &str = "ok";

const SECCOMP_DATA_NR: u32 = 0;
const SECCOMP_DATA_ARCH: u32 = 4;
const LD_W_ABS: u16 = 0x20;
const JMP_JEQ_K: u16 = 0x15;
const RET_K: u16 = 0x06;
const RET_ALLOW: u32 = 0x7fff_0000;
const RET_USER_NOTIF: u32 = 0x7fc0_0000;
const RET_KILL_PROCESS: u32 = 0x8000_0000;

/// `AUDIT_ARCH_*` and the syscall numbers routed to the guard on that arch.
#[derive(Debug, Clone, Copy)]
pub struct Abi {
    pub arch: u32,
    pub traced: &'static [u32],
}

const X32_SYSCALL_BIT: u32 = 0x4000_0000;

/// x86-64 `execve`, `execveat`, and their x32 numbers (same audit arch).
pub const X86_64: Abi = Abi {
    arch: 0xC000_003E,
    traced: &[59, 322, X32_SYSCALL_BIT | 520, X32_SYSCALL_BIT | 545],
};

/// aarch64 `execve`, `execveat`.
pub const AARCH64: Abi = Abi { arch: 0xC000_00B7, traced: &[221, 281] };

/// The build target's ABI, or `None` on an unsupported arch.
#[must_use]
pub const fn native() -> Option<Abi> {
    if cfg!(target_arch = "x86_64") {
        Some(X86_64)
    } else if cfg!(target_arch = "aarch64") {
        Some(AARCH64)
    } else {
        None
    }
}

/// One classic-BPF instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Insn {
    pub code: u16,
    pub jt: u8,
    pub jf: u8,
    pub k: u32,
}

const fn stmt(code: u16, k: u32) -> Insn {
    Insn { code, jt: 0, jf: 0, k }
}

/// The filter: a foreign arch kills the process (a compat-mode exec would
/// otherwise go unseen), a traced syscall goes to the guard, everything else
/// is allowed.
#[must_use]
pub fn program(abi: Abi) -> Vec<Insn> {
    let n = abi.traced.len();
    let mut prog = vec![
        stmt(LD_W_ABS, SECCOMP_DATA_ARCH),
        Insn { code: JMP_JEQ_K, jt: 1, jf: 0, k: abi.arch },
        stmt(RET_K, RET_KILL_PROCESS),
        stmt(LD_W_ABS, SECCOMP_DATA_NR),
    ];
    for (i, &nr) in abi.traced.iter().enumerate() {
        prog.push(Insn { code: JMP_JEQ_K, jt: (n - i) as u8, jf: 0, k: nr });
    }
    prog.push(stmt(RET_K, RET_ALLOW));
    prog.push(stmt(RET_K, RET_USER_NOTIF));
    prog
}

/// Set `no_new_privs` and install [`program`] on the calling thread, returning
/// the user-notification listener fd. The process must still be
/// single-threaded.
pub fn install() -> Result<OwnedFd> {
    let Some(abi) = native() else {
        bail!("exec interception: unsupported architecture");
    };
    nix::sys::prctl::set_no_new_privs().context("setting no_new_privs")?;
    sys::install_listener(&program(abi)).context("seccomp(SECCOMP_FILTER_FLAG_NEW_LISTENER)")
}

/// Send `listener` to the guard at `socket` and wait for its verdict on the
/// hand-off. The caller's copy is closed on return either way.
pub fn hand_off(socket: &Path, listener: OwnedFd, timeout: Duration) -> Result<()> {
    let stream = UnixStream::connect(socket)
        .with_context(|| format!("connecting to guard exec socket {}", socket.display()))?;
    stream.set_read_timeout(Some(timeout)).context("set_read_timeout")?;
    stream.set_write_timeout(Some(timeout)).context("set_write_timeout")?;
    sys::send_fd(&stream, listener.as_fd()).context("sending listener fd")?;
    drop(listener);
    let mut reply = Vec::new();
    (&stream).take(4096).read_to_end(&mut reply).context("reading guard hand-off reply")?;
    parse_ack(&reply)
}

/// Interpret the guard's hand-off reply: [`ACK_OK`] or a refusal reason.
pub fn parse_ack(reply: &[u8]) -> Result<()> {
    let text = String::from_utf8_lossy(reply);
    let text = text.trim();
    if text == ACK_OK {
        return Ok(());
    }
    if text.is_empty() {
        bail!("guard closed the exec hand-off without accepting it");
    }
    bail!("guard refused the exec hand-off: {text}");
}

#[allow(unsafe_code)]
mod sys {
    use super::{AsRawFd, BorrowedFd, FromRawFd, Insn, OwnedFd, RawFd, UnixStream};

    pub fn install_listener(prog: &[Insn]) -> std::io::Result<OwnedFd> {
        let mut filters: Vec<libc::sock_filter> = prog
            .iter()
            .map(|i| libc::sock_filter { code: i.code, jt: i.jt, jf: i.jf, k: i.k })
            .collect();
        let fprog = libc::sock_fprog {
            len: u16::try_from(filters.len()).map_err(|_| std::io::ErrorKind::InvalidInput)?,
            filter: filters.as_mut_ptr(),
        };
        // SAFETY: `fprog` points at `filters`, both alive for the call; the
        // kernel copies the program before returning.
        let ret = unsafe {
            libc::syscall(
                libc::SYS_seccomp,
                libc::SECCOMP_SET_MODE_FILTER,
                libc::SECCOMP_FILTER_FLAG_NEW_LISTENER,
                &raw const fprog,
            )
        };
        if ret < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let fd = RawFd::try_from(ret).map_err(|_| std::io::ErrorKind::InvalidData)?;
        // SAFETY: the kernel just returned this fd and nothing else owns it.
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    pub fn send_fd(stream: &UnixStream, fd: BorrowedFd<'_>) -> std::io::Result<()> {
        let mut payload = [b'F'];
        let mut iov = libc::iovec { iov_base: payload.as_mut_ptr().cast(), iov_len: 1 };
        let mut control = [0u64; 8];
        let fd_len = std::mem::size_of::<RawFd>() as libc::c_uint;
        // SAFETY: pure size arithmetic.
        let space = unsafe { libc::CMSG_SPACE(fd_len) } as usize;
        // SAFETY: an all-zero msghdr is a valid empty message.
        let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
        msg.msg_iov = &raw mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = control.as_mut_ptr().cast();
        msg.msg_controllen = space as _;
        // SAFETY: `control` is 64 bytes, u64-aligned, and `space` (<= 24) fits
        // in it, so the first header and its data are in bounds.
        unsafe {
            let cmsg = libc::CMSG_FIRSTHDR(&raw const msg);
            if cmsg.is_null() {
                return Err(std::io::ErrorKind::InvalidInput.into());
            }
            (*cmsg).cmsg_level = libc::SOL_SOCKET;
            (*cmsg).cmsg_type = libc::SCM_RIGHTS;
            (*cmsg).cmsg_len = libc::CMSG_LEN(fd_len) as _;
            std::ptr::write_unaligned(libc::CMSG_DATA(cmsg).cast::<RawFd>(), fd.as_raw_fd());
        }
        // SAFETY: `msg` and everything it points at outlive the call.
        let sent = unsafe { libc::sendmsg(stream.as_raw_fd(), &raw const msg, libc::MSG_NOSIGNAL) };
        if sent < 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixListener;

    use super::*;

    /// Minimal classic-BPF interpreter for the opcodes [`program`] emits.
    fn eval(prog: &[Insn], arch: u32, nr: u32) -> u32 {
        let mut acc = 0;
        let mut pc = 0;
        loop {
            let i = prog[pc];
            match i.code {
                LD_W_ABS => {
                    acc = if i.k == SECCOMP_DATA_ARCH { arch } else { nr };
                    pc += 1;
                }
                JMP_JEQ_K => pc += 1 + usize::from(if acc == i.k { i.jt } else { i.jf }),
                RET_K => return i.k,
                other => panic!("unexpected opcode {other:#x}"),
            }
        }
    }

    #[test]
    fn x86_64_routes_execve_and_execveat_to_the_guard() {
        let prog = program(X86_64);
        for nr in X86_64.traced {
            assert_eq!(eval(&prog, X86_64.arch, *nr), RET_USER_NOTIF, "nr {nr}");
        }
    }

    #[test]
    fn other_syscalls_are_allowed() {
        let prog = program(X86_64);
        for nr in [0, 1, 2, 57, 56, 435, 231] {
            assert_eq!(eval(&prog, X86_64.arch, nr), RET_ALLOW, "nr {nr}");
        }
        let prog = program(AARCH64);
        assert_eq!(eval(&prog, AARCH64.arch, 220), RET_ALLOW);
        assert_eq!(eval(&prog, AARCH64.arch, 221), RET_USER_NOTIF);
        assert_eq!(eval(&prog, AARCH64.arch, 281), RET_USER_NOTIF);
    }

    #[test]
    fn foreign_arch_is_killed() {
        let i386 = 0x4000_0003;
        assert_eq!(eval(&program(X86_64), i386, 11), RET_KILL_PROCESS);
        assert_eq!(eval(&program(AARCH64), 0x4000_0028, 11), RET_KILL_PROCESS);
    }

    #[test]
    fn ack_ok_is_accepted() {
        parse_ack(b"ok\n").unwrap();
    }

    #[test]
    fn refusal_and_silence_are_errors() {
        let err = parse_ack(b"refused: guard lacks CAP_SYS_PTRACE\n").unwrap_err();
        assert!(err.to_string().contains("CAP_SYS_PTRACE"), "{err}");
        assert!(parse_ack(b"").is_err());
        assert!(parse_ack(b"OK maybe").is_err());
    }

    fn fake_guard(reply: &'static [u8]) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("exec.sock");
        let listener = UnixListener::bind(&path).unwrap();
        std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let mut byte = [0u8; 1];
            conn.read_exact(&mut byte).unwrap();
            conn.write_all(reply).unwrap();
        });
        (dir, path)
    }

    fn some_fd() -> OwnedFd {
        std::fs::File::open("/dev/null").unwrap().into()
    }

    #[test]
    fn hand_off_succeeds_on_ack() {
        let (_dir, path) = fake_guard(b"ok\n");
        hand_off(&path, some_fd(), Duration::from_secs(5)).unwrap();
    }

    #[test]
    fn hand_off_fails_on_refusal() {
        let (_dir, path) = fake_guard(b"refused: no policy\n");
        let err = hand_off(&path, some_fd(), Duration::from_secs(5)).unwrap_err();
        assert!(format!("{err:#}").contains("no policy"), "{err:#}");
    }

    #[test]
    fn hand_off_fails_without_a_guard() {
        let dir = tempfile::tempdir().unwrap();
        assert!(hand_off(&dir.path().join("absent.sock"), some_fd(), HANDOFF_TIMEOUT).is_err());
    }
}
