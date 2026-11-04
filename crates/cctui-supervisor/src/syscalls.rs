//! Per-architecture syscall-number tables for the seccomp denylist.
//!
//! The numbers are looked up per target arch so the emitted BPF program
//! matches the kernel's syscall ABI exactly.

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

/// `personality(2)` personas the filter lets through, as Docker's default
/// profile does: the `0xffffffff` query, `PER_LINUX`, `PER_LINUX32` and their
/// `UNAME26` variants. Any other persona (`ADDR_NO_RANDOMIZE`,
/// `READ_IMPLIES_EXEC`, ...) weakens exploit mitigations and is denied.
pub const PERSONALITY_ALLOWED: &[u64] = &[0x0, 0x8, 0x2_0000, 0x2_0008, 0xFFFF_FFFF];

/// Persona values to let through for an argument-guarded `personality`, or
/// `None` for every other syscall.
#[must_use]
pub fn allowed_personas(name: &str) -> Option<&'static [u64]> {
    match name {
        "personality" => Some(PERSONALITY_ALLOWED),
        _ => None,
    }
}

/// Denied syscalls and why. Order is fixed for deterministic output.
///
/// The `io_uring` calls are denied with `EPERM`, which libuv (node, claude) and
/// tokio treat as "`io_uring` unavailable" and fall back to their thread pools,
/// exactly as under Docker's default profile.
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
    // Kernel attack surface
    ("bpf", "load eBPF programs / maps"),
    ("perf_event_open", "kernel performance monitoring"),
    ("userfaultfd", "page-fault handling used to win kernel races"),
    ("io_uring_setup", "io_uring bypasses the seccomp filter for its ops"),
    ("io_uring_enter", "io_uring bypasses the seccomp filter for its ops"),
    ("io_uring_register", "io_uring bypasses the seccomp filter for its ops"),
    ("open_by_handle_at", "open files outside the mount namespace by handle"),
    ("personality", "disable ASLR / enable READ_IMPLIES_EXEC"),
    // Kernel keyring
    ("keyctl", "manipulate kernel keyrings"),
    ("add_key", "add a key to a kernel keyring"),
    ("request_key", "request a key from a kernel keyring"),
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
    ("bpf", 321),
    ("perf_event_open", 298),
    ("userfaultfd", 323),
    ("io_uring_setup", 425),
    ("io_uring_enter", 426),
    ("io_uring_register", 427),
    ("open_by_handle_at", 304),
    ("personality", 135),
    ("keyctl", 250),
    ("add_key", 248),
    ("request_key", 249),
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
    ("bpf", 280),
    ("perf_event_open", 241),
    ("userfaultfd", 282),
    ("io_uring_setup", 425),
    ("io_uring_enter", 426),
    ("io_uring_register", 427),
    ("open_by_handle_at", 265),
    ("personality", 92),
    ("keyctl", 219),
    ("add_key", 217),
    ("request_key", 218),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denylist_has_expected_entries() {
        assert_eq!(DENYLIST.len(), 32);
        for name in [
            "unshare",
            "ptrace",
            "pivot_root",
            "chroot",
            "bpf",
            "keyctl",
            "add_key",
            "request_key",
            "userfaultfd",
            "perf_event_open",
            "io_uring_setup",
            "io_uring_enter",
            "io_uring_register",
            "open_by_handle_at",
            "personality",
        ] {
            assert!(DENYLIST.iter().any(|(n, _)| *n == name), "{name} must be denied");
        }
    }

    #[test]
    fn denylist_names_are_unique() {
        let mut names: Vec<&str> = DENYLIST.iter().map(|(n, _)| *n).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), DENYLIST.len());
    }

    #[test]
    fn only_personality_is_persona_guarded() {
        assert_eq!(allowed_personas("personality"), Some(PERSONALITY_ALLOWED));
        assert!(PERSONALITY_ALLOWED.contains(&0xFFFF_FFFF), "the query must stay allowed");
        assert!(!PERSONALITY_ALLOWED.contains(&0x0004_0000), "ADDR_NO_RANDOMIZE must be denied");
        for (name, _) in DENYLIST.iter().filter(|(n, _)| *n != "personality") {
            assert_eq!(allowed_personas(name), None, "{name}");
        }
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
        assert_eq!(find("bpf"), 321);
        assert_eq!(find("keyctl"), 250);
        assert_eq!(find("add_key"), 248);
        assert_eq!(find("request_key"), 249);
        assert_eq!(find("userfaultfd"), 323);
        assert_eq!(find("perf_event_open"), 298);
        assert_eq!(find("io_uring_setup"), 425);
        assert_eq!(find("io_uring_enter"), 426);
        assert_eq!(find("io_uring_register"), 427);
        assert_eq!(find("open_by_handle_at"), 304);
        assert_eq!(find("personality"), 135);
    }

    #[cfg(target_arch = "aarch64")]
    #[test]
    fn arm64_known_numbers() {
        let blocked = blocked_syscalls().unwrap();
        let find = |name: &str| blocked.iter().find(|b| b.name == name).unwrap().num;
        assert_eq!(find("bpf"), 280);
        assert_eq!(find("keyctl"), 219);
        assert_eq!(find("userfaultfd"), 282);
        assert_eq!(find("perf_event_open"), 241);
        assert_eq!(find("io_uring_setup"), 425);
        assert_eq!(find("open_by_handle_at"), 265);
        assert_eq!(find("personality"), 92);
    }
}
