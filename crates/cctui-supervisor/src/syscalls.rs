//! Per-architecture syscall-number tables for the seccomp denylist.
//!
//! Faithful port of `syscalls_amd64.go` / `syscalls_arm64.go` from the Go
//! reference. The `(name, reason)` pairs come from `seccomp.go`'s
//! `blockedSyscalls` map; the numbers are looked up per target arch so the
//! emitted BPF program matches the kernel's syscall ABI exactly.

/// A blocked syscall: stable name, the reason it is denied (for the report /
/// logging), and its number on the current target architecture.
#[derive(Debug, Clone, Copy)]
pub struct Blocked {
    pub name: &'static str,
    pub reason: &'static str,
    pub num: i64,
}

/// Ordered list of denied syscalls with their numbers for the current target
/// architecture. Ordering is fixed (not a hash map) so the report and the
/// emitted filter are deterministic.
///
/// Returns `None` on architectures we do not have a number table for (mirrors
/// the Go `archToken`/`syscallNumbers` "unsupported architecture" path).
#[must_use]
pub fn blocked_syscalls() -> Option<Vec<Blocked>> {
    let table = arch_table()?;
    Some(
        DENYLIST
            .iter()
            .map(|&(name, reason)| {
                let num = table
                    .iter()
                    .find(|&&(n, _)| n == name)
                    .map(|&(_, num)| num)
                    .expect("every denied syscall must have an arch number");
                Blocked { name, reason, num }
            })
            .collect(),
    )
}

/// Number of leading id arguments to guard for a uid/gid-setting syscall.
///
/// Returns the count of `uid_t`/`gid_t` arguments the seccomp filter must guard
/// for the given syscall, or `None` for syscalls that are blocked
/// unconditionally.
///
/// These calls are not denied outright: a *no-op* to the identity the payload
/// already runs as (or the `-1` "leave unchanged" sentinel) is permitted, while
/// any attempt to switch to a different uid/gid still returns `EPERM`. This is
/// required because GNU Make's recipe-spawn child resets its effective uid to
/// the real uid (`setresuid(-1, <uid>, -1)`) before `execve`; a blanket block
/// turned that no-op into `EPERM`, killing the recipe shell with exit 127.
/// A no-op reset grants no privilege, so allowing it does not weaken
/// the sandbox — `setuid(0)` and friends stay denied.
#[must_use]
pub fn id_setter_argc(name: &str) -> Option<u8> {
    match name {
        "setuid" | "setgid" => Some(1),
        "setreuid" | "setregid" => Some(2),
        "setresuid" | "setresgid" => Some(3),
        _ => None,
    }
}

/// Names + reasons, ported verbatim from `seccomp.go` `blockedSyscalls`.
/// Order is fixed here (the Go map is unordered) for deterministic output.
const DENYLIST: &[(&str, &str)] = &[
    // Process debugging / escape
    ("ptrace", "attach to other processes"),
    ("process_vm_readv", "read another process's memory"),
    ("process_vm_writev", "write another process's memory"),
    // Privilege escalation
    ("setuid", "change user ID"),
    ("setgid", "change group ID"),
    ("setreuid", "change real/effective UID"),
    ("setregid", "change real/effective GID"),
    ("setresuid", "change real/saved/effective UID"),
    ("setresgid", "change real/saved/effective GID"),
    // Filesystem escape
    ("mount", "mount filesystems"),
    ("umount2", "unmount filesystems"),
    ("pivot_root", "change root filesystem"),
    ("chroot", "change root directory"),
    // Kernel manipulation
    ("reboot", "reboot the system"),
    ("kexec_load", "load a new kernel"),
    ("kexec_file_load", "load a new kernel from file"),
    ("init_module", "load kernel module"),
    ("finit_module", "load kernel module from fd"),
    ("delete_module", "unload kernel module"),
    // Namespace escape
    ("unshare", "create new namespaces"),
    ("setns", "enter another namespace"),
];

/// Returns the `(name, number)` table for the build target architecture, or
/// `None` if unsupported.
// The Option is load-bearing on unsupported arches (the `cfg(not(...))` arm);
// clippy only sees one cfg arm at a time, hence the allows.
#[allow(clippy::unnecessary_wraps, clippy::missing_const_for_fn)]
fn arch_table() -> Option<&'static [(&'static str, i64)]> {
    #[cfg(target_arch = "x86_64")]
    {
        Some(AMD64)
    }
    #[cfg(target_arch = "aarch64")]
    {
        Some(ARM64)
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        None
    }
}

/// `x86_64` syscall numbers. Source: `linux/arch/x86/entry/syscalls/syscall_64.tbl`.
/// Port of `syscalls_amd64.go`.
#[cfg(target_arch = "x86_64")]
const AMD64: &[(&str, i64)] = &[
    ("ptrace", 101),
    ("process_vm_readv", 310),
    ("process_vm_writev", 311),
    ("setuid", 105),
    ("setgid", 106),
    ("setreuid", 113),
    ("setregid", 114),
    ("setresuid", 117),
    ("setresgid", 119),
    ("mount", 165),
    ("umount2", 166),
    ("pivot_root", 155),
    ("chroot", 161),
    ("reboot", 169),
    ("kexec_load", 246),
    ("kexec_file_load", 320),
    ("init_module", 175),
    ("finit_module", 313),
    ("delete_module", 176),
    ("unshare", 272),
    ("setns", 308),
];

/// arm64 syscall numbers. Source: `linux/include/uapi/asm-generic/unistd.h`
/// (arm64 uses the generic table). Port of `syscalls_arm64.go`.
#[cfg(target_arch = "aarch64")]
const ARM64: &[(&str, i64)] = &[
    ("ptrace", 117),
    ("process_vm_readv", 270),
    ("process_vm_writev", 271),
    ("setuid", 146),
    ("setgid", 144),
    ("setreuid", 145),
    ("setregid", 143),
    ("setresuid", 147),
    ("setresgid", 149),
    ("mount", 40),
    ("umount2", 39),
    ("pivot_root", 41),
    ("chroot", 51),
    ("reboot", 142),
    ("kexec_load", 104),
    ("kexec_file_load", 294),
    ("init_module", 105),
    ("finit_module", 273),
    ("delete_module", 106),
    ("unshare", 97),
    ("setns", 268),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denylist_has_expected_entries() {
        // Same 21 syscalls as the Go reference's blockedSyscalls map.
        assert_eq!(DENYLIST.len(), 21);
        assert!(DENYLIST.iter().any(|(n, _)| *n == "unshare"));
        assert!(DENYLIST.iter().any(|(n, _)| *n == "ptrace"));
        assert!(DENYLIST.iter().any(|(n, _)| *n == "pivot_root"));
        assert!(DENYLIST.iter().any(|(n, _)| *n == "chroot"));
    }

    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    #[test]
    fn every_denied_syscall_resolves_to_a_number() {
        let blocked = blocked_syscalls().expect("supported arch");
        assert_eq!(blocked.len(), DENYLIST.len());
        // Numbers must be distinct on this arch.
        let mut nums: Vec<i64> = blocked.iter().map(|b| b.num).collect();
        nums.sort_unstable();
        nums.dedup();
        assert_eq!(nums.len(), blocked.len());
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn amd64_known_numbers() {
        let blocked = blocked_syscalls().unwrap();
        let find = |name: &str| blocked.iter().find(|b| b.name == name).unwrap().num;
        assert_eq!(find("ptrace"), 101);
        assert_eq!(find("unshare"), 272);
        assert_eq!(find("mount"), 165);
    }
}
