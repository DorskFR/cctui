#![allow(unsafe_code)]

use std::io;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;

#[derive(Debug, Clone, Copy)]
pub struct Notif {
    pub id: u64,
    pub pid: u32,
    pub nr: i32,
    pub arch: u32,
    pub args: [u64; 6],
}

/// Buffer sizes for the notification ioctls: the kernel's, when larger than
/// the structs this build knows, so a newer kernel never writes past them.
#[derive(Debug, Clone, Copy)]
pub struct Sizes {
    notif: usize,
    resp: usize,
}

fn check(ret: libc::c_long) -> io::Result<libc::c_long> {
    if ret < 0 { Err(io::Error::last_os_error()) } else { Ok(ret) }
}

fn owned(ret: libc::c_long) -> io::Result<OwnedFd> {
    let fd = RawFd::try_from(check(ret)?).map_err(|_| io::ErrorKind::InvalidData)?;
    // SAFETY: the kernel just returned this fd and nothing else owns it.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

pub fn notif_sizes() -> io::Result<Sizes> {
    let mut sizes =
        libc::seccomp_notif_sizes { seccomp_notif: 0, seccomp_notif_resp: 0, seccomp_data: 0 };
    // SAFETY: the kernel fills `sizes`, which outlives the call.
    check(unsafe {
        libc::syscall(libc::SYS_seccomp, libc::SECCOMP_GET_NOTIF_SIZES, 0u32, &raw mut sizes)
    })?;
    Ok(Sizes {
        notif: usize::from(sizes.seccomp_notif).max(size_of::<libc::seccomp_notif>()),
        resp: usize::from(sizes.seccomp_notif_resp).max(size_of::<libc::seccomp_notif_resp>()),
    })
}

pub fn recv(listener: BorrowedFd<'_>, sizes: Sizes) -> io::Result<Notif> {
    let mut buf = vec![0u64; sizes.notif.div_ceil(8)];
    // SAFETY: `buf` is zeroed (the kernel requires it), 8-byte aligned and at
    // least the kernel's notification size.
    let ret = unsafe {
        libc::ioctl(listener.as_raw_fd(), libc::SECCOMP_IOCTL_NOTIF_RECV, buf.as_mut_ptr())
    };
    check(libc::c_long::from(ret))?;
    // SAFETY: `buf` holds at least one `seccomp_notif`, just written.
    let raw: libc::seccomp_notif = unsafe { std::ptr::read_unaligned(buf.as_ptr().cast()) };
    Ok(Notif {
        id: raw.id,
        pid: raw.pid,
        nr: raw.data.nr,
        arch: raw.data.arch,
        args: raw.data.args,
    })
}

/// Answer notification `id`: let the exec proceed, or fail it with `EPERM`.
pub fn respond(listener: BorrowedFd<'_>, sizes: Sizes, id: u64, allow: bool) -> io::Result<()> {
    let resp = libc::seccomp_notif_resp {
        id,
        val: 0,
        error: if allow { 0 } else { -libc::EPERM },
        flags: if allow { libc::SECCOMP_USER_NOTIF_FLAG_CONTINUE as u32 } else { 0 },
    };
    let mut buf = vec![0u64; sizes.resp.div_ceil(8)];
    // SAFETY: `buf` is at least one `seccomp_notif_resp` long.
    unsafe { std::ptr::write_unaligned(buf.as_mut_ptr().cast::<libc::seccomp_notif_resp>(), resp) };
    // SAFETY: `buf` outlives the call; the kernel only reads it.
    let ret = unsafe {
        libc::ioctl(listener.as_raw_fd(), libc::SECCOMP_IOCTL_NOTIF_SEND, buf.as_mut_ptr())
    };
    check(libc::c_long::from(ret)).map(drop)
}

/// Whether notification `id` is still pending: the target has not died and
/// its pid was not reused.
pub fn id_valid(listener: BorrowedFd<'_>, id: u64) -> bool {
    // SAFETY: the kernel reads one u64 from `id`.
    let ret = unsafe {
        libc::ioctl(listener.as_raw_fd(), libc::SECCOMP_IOCTL_NOTIF_ID_VALID, &raw const id)
    };
    ret == 0
}

pub fn pidfd_open(pid: u32) -> io::Result<OwnedFd> {
    let pid = libc::pid_t::try_from(pid).map_err(|_| io::ErrorKind::InvalidInput)?;
    // SAFETY: plain syscall with scalar arguments.
    owned(unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0u32) })
}

/// Duplicate `target_fd` of the process behind `pidfd` into this process.
pub fn pidfd_getfd(pidfd: BorrowedFd<'_>, target_fd: RawFd) -> io::Result<OwnedFd> {
    // SAFETY: plain syscall with scalar arguments.
    owned(unsafe { libc::syscall(libc::SYS_pidfd_getfd, pidfd.as_raw_fd(), target_fd, 0u32) })
}

pub fn is_writable(fd: BorrowedFd<'_>) -> bool {
    // SAFETY: F_GETFL only reads the descriptor's flags.
    let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
    flags >= 0 && matches!(flags & libc::O_ACCMODE, libc::O_WRONLY | libc::O_RDWR)
}

pub fn effective_uid() -> u32 {
    // SAFETY: geteuid cannot fail.
    unsafe { libc::geteuid() }
}

pub fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: `cred` and `len` outlive the call and `len` is its size.
    let ret = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&raw mut cred).cast(),
            &raw mut len,
        )
    };
    check(libc::c_long::from(ret))?;
    Ok(cred.uid)
}

/// Receive exactly one fd sent with `SCM_RIGHTS`; any extra fds are closed.
pub fn recv_fd(stream: &UnixStream) -> io::Result<OwnedFd> {
    let mut payload = [0u8; 1];
    let mut iov = libc::iovec { iov_base: payload.as_mut_ptr().cast(), iov_len: 1 };
    let mut control = [0u64; 8];
    // SAFETY: an all-zero msghdr is a valid empty message.
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &raw mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr().cast();
    msg.msg_controllen = size_of_val(&control) as _;
    // SAFETY: `msg` and its buffers outlive the call.
    let n = unsafe { libc::recvmsg(stream.as_raw_fd(), &raw mut msg, libc::MSG_CMSG_CLOEXEC) };
    check(n as libc::c_long)?;
    let mut fds = Vec::new();
    // SAFETY: the cmsg walk stays within `control` via the CMSG_* helpers, and
    // each SCM_RIGHTS fd is a fresh descriptor the kernel installed for us.
    unsafe {
        let mut cmsg = libc::CMSG_FIRSTHDR(&raw const msg);
        while !cmsg.is_null() {
            if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                let data = libc::CMSG_DATA(cmsg).cast::<RawFd>();
                let bytes = ((*cmsg).cmsg_len as libc::size_t)
                    .saturating_sub(libc::CMSG_LEN(0) as libc::size_t);
                for i in 0..bytes / size_of::<RawFd>() {
                    fds.push(OwnedFd::from_raw_fd(std::ptr::read_unaligned(data.add(i))));
                }
            }
            cmsg = libc::CMSG_NXTHDR(&raw const msg, cmsg);
        }
    }
    if msg.msg_flags & libc::MSG_CTRUNC != 0 || fds.len() != 1 {
        return Err(io::Error::other(format!("expected one fd, got {}", fds.len())));
    }
    Ok(fds.remove(0))
}
