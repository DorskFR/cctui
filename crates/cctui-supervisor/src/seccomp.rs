//! Seccomp-BPF denylist: `EPERM` for a fixed set of dangerous syscalls,
//! everything else allowed. `seccompiler` emits the program (arch guard,
//! per-syscall comparisons, default allow) and `apply_filter` sets
//! `PR_SET_NO_NEW_PRIVS` before installing it.

use anyhow::{Context, Result, anyhow};
use seccompiler::{
    BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
    SeccompRule, TargetArch,
};

use crate::syscalls::{allowed_personas, blocked_syscalls, id_setter_argc};

/// The `uid_t`/`gid_t` "leave this field unchanged" sentinel, as seen by seccomp
/// in the (32-bit) argument register.
const ID_UNCHANGED: u64 = 0xFFFF_FFFF;

/// Resolve the seccompiler target arch for the build target. Mirrors the Go
/// `archToken()` switch (unsupported arch is an error).
// The Result is load-bearing on the `cfg(not(...))` arm; clippy only sees one
// cfg arm at a time.
#[allow(clippy::unnecessary_wraps, clippy::missing_const_for_fn)]
fn target_arch() -> Result<TargetArch> {
    #[cfg(target_arch = "x86_64")]
    {
        Ok(TargetArch::x86_64)
    }
    #[cfg(target_arch = "aarch64")]
    {
        Ok(TargetArch::aarch64)
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        Err(anyhow!("unsupported architecture for seccomp"))
    }
}

/// Rules for a uid/gid-setting syscall that return `EPERM` for any real
/// identity *change* while letting a no-op through to the default `Allow`.
///
/// `argc` id arguments are guarded. Each argument gets its own rule, and the
/// rules are combined with logical OR: "this argument is neither the worker id
/// nor the `-1` unchanged sentinel". If any argument tries to switch to a
/// different id, its rule matches and the call is denied; if every argument is
/// the worker id or `-1`, no rule matches and the (harmless) no-op falls
/// through to `Allow`.
///
/// Conditions within a rule are combined with logical AND, so
/// `[Ne worker, Ne unchanged]` means "a value other than worker and other than
/// unchanged". Comparisons use `Dword` (the low 32 bits) because
/// `uid_t`/`gid_t` are 32-bit.
fn id_setter_rules(argc: u8, worker_id: u64) -> Result<Vec<SeccompRule>> {
    (0..argc)
        .map(|arg_index| {
            SeccompRule::new(vec![
                SeccompCondition::new(
                    arg_index,
                    SeccompCmpArgLen::Dword,
                    SeccompCmpOp::Ne,
                    worker_id & ID_UNCHANGED,
                )
                .context("seccomp: worker-id condition")?,
                SeccompCondition::new(
                    arg_index,
                    SeccompCmpArgLen::Dword,
                    SeccompCmpOp::Ne,
                    ID_UNCHANGED,
                )
                .context("seccomp: unchanged-sentinel condition")?,
            ])
            .context("seccomp: id-setter rule")
        })
        .collect()
}

/// One rule denying `personality` unless its persona argument is one of
/// `allowed` (conditions within a rule are ANDed: "none of the allowed values").
fn persona_rule(allowed: &[u64]) -> Result<Vec<SeccompRule>> {
    let conditions = allowed
        .iter()
        .map(|&persona| {
            SeccompCondition::new(0, SeccompCmpArgLen::Dword, SeccompCmpOp::Ne, persona)
                .context("seccomp: persona condition")
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(vec![SeccompRule::new(conditions).context("seccomp: persona rule")?])
}

/// Build the denylist BPF program for a payload running as `worker_id` (used as
/// both the uid and gid the worker is dropped to).
///
/// Each blocked syscall returns `EPERM`; every other syscall is allowed (the
/// mismatch action). The uid/gid-setting syscalls are guarded conditionally so
/// a no-op reset to `worker_id` (or `-1`) is permitted while real identity
/// changes stay denied. Returns the program and the ordered list of
/// blocked syscall names (for logging / the report).
pub fn build(worker_id: u32) -> Result<(BpfProgram, Vec<&'static str>)> {
    let arch = target_arch()?;
    let blocked =
        blocked_syscalls().ok_or_else(|| anyhow!("no syscall number table for this arch"))?;

    let mut rules = std::collections::BTreeMap::new();
    for b in &blocked {
        // Empty rule vec => match unconditionally (blanket EPERM). The
        // id-setting syscalls instead get argument-guarded rules so a no-op
        // reset is allowed but any privilege change is denied.
        let syscall_rules = match (id_setter_argc(b.name), allowed_personas(b.name)) {
            (Some(argc), _) => id_setter_rules(argc, u64::from(worker_id))?,
            (None, Some(allowed)) => persona_rule(allowed)?,
            (None, None) => vec![],
        };
        rules.insert(b.num, syscall_rules);
    }

    let filter = SeccompFilter::new(
        rules,
        SeccompAction::Allow,                     // default: allow
        SeccompAction::Errno(libc::EPERM as u32), // blocked: EPERM
        arch,
    )
    .context("building seccomp filter")?;

    let program: BpfProgram = filter.try_into().context("compiling seccomp filter")?;
    let names = blocked.iter().map(|b| b.name).collect();
    Ok((program, names))
}

/// Build and install the denylist on the calling thread for a payload running
/// as `worker_id`. Returns the blocked syscall names. `apply_filter` sets
/// `NO_NEW_PRIVS` internally.
pub fn apply(worker_id: u32) -> Result<Vec<&'static str>> {
    let (program, names) = build(worker_id)?;
    seccompiler::apply_filter(&program).context("installing seccomp filter")?;
    Ok(names)
}

#[cfg(all(test, any(target_arch = "x86_64", target_arch = "aarch64")))]
mod tests {
    use seccompiler::sock_filter;

    use super::*;
    use crate::syscalls::PERSONALITY_ALLOWED;

    const RET_ALLOW: u32 = 0x7fff_0000;
    const RET_EPERM: u32 = 0x0005_0000 | 1;
    const WORKER: u32 = 1000;

    #[cfg(target_arch = "x86_64")]
    const AUDIT_ARCH: u32 = 62 | 0x8000_0000 | 0x4000_0000;
    #[cfg(target_arch = "aarch64")]
    const AUDIT_ARCH: u32 = 183 | 0x8000_0000 | 0x4000_0000;

    /// `struct seccomp_data` as the kernel lays it out (little-endian).
    fn seccomp_data(nr: i64, args: [u64; 6]) -> Vec<u8> {
        let mut data = Vec::with_capacity(64);
        data.extend_from_slice(&i32::try_from(nr).unwrap().to_le_bytes());
        data.extend_from_slice(&AUDIT_ARCH.to_le_bytes());
        data.extend_from_slice(&0u64.to_le_bytes());
        for a in args {
            data.extend_from_slice(&a.to_le_bytes());
        }
        data
    }

    /// Minimal classic-BPF interpreter covering the opcodes seccompiler emits.
    /// An unknown opcode panics so the test can never silently pass.
    fn run(program: &[sock_filter], data: &[u8]) -> u32 {
        let load = |k: u32| {
            let at = usize::try_from(k).unwrap();
            u32::from_le_bytes(data[at..at + 4].try_into().unwrap())
        };
        let (mut pc, mut acc) = (0usize, 0u32);
        loop {
            let (code, jt, jf, k) = {
                let ins = &program[pc];
                (ins.code, ins.jt, ins.jf, ins.k)
            };
            pc += 1;
            let jump = |taken: bool| usize::from(if taken { jt } else { jf });
            match code {
                0x20 => acc = load(k),
                0x54 => acc &= k,
                0x05 => pc += usize::try_from(k).unwrap(),
                0x15 => pc += jump(acc == k),
                0x25 => pc += jump(acc > k),
                0x35 => pc += jump(acc >= k),
                0x06 => return k,
                other => panic!("unexpected BPF opcode {other:#x} at {}", pc - 1),
            }
        }
    }

    fn verdict(name: &str, args: [u64; 6]) -> u32 {
        let (program, _) = build(WORKER).unwrap();
        let nr = blocked_syscalls()
            .unwrap()
            .into_iter()
            .find(|b| b.name == name)
            .map_or_else(|| allowed_nr(name), |b| b.num);
        run(&program, &seccomp_data(nr, args))
    }

    #[cfg(target_arch = "x86_64")]
    fn allowed_nr(name: &str) -> i64 {
        match name {
            "read" => 0,
            "write" => 1,
            "mmap" => 9,
            "mprotect" => 10,
            "clone" => 56,
            "execve" => 59,
            "epoll_pwait" => 281,
            "openat" => 257,
            "futex" => 202,
            "clone3" => 435,
            "landlock_create_ruleset" => 444,
            other => panic!("no number for {other}"),
        }
    }

    #[cfg(target_arch = "aarch64")]
    fn allowed_nr(name: &str) -> i64 {
        match name {
            "read" => 63,
            "write" => 64,
            "mmap" => 222,
            "mprotect" => 226,
            "clone" => 220,
            "execve" => 221,
            "epoll_pwait" => 22,
            "openat" => 56,
            "futex" => 98,
            "clone3" => 435,
            "landlock_create_ruleset" => 444,
            other => panic!("no number for {other}"),
        }
    }

    #[test]
    fn new_kernel_surface_syscalls_are_denied() {
        for name in [
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
        ] {
            assert_eq!(verdict(name, [0; 6]), RET_EPERM, "{name} must return EPERM");
        }
    }

    #[test]
    fn existing_denials_still_hold() {
        for name in ["ptrace", "mount", "unshare", "setns", "chroot", "init_module"] {
            assert_eq!(verdict(name, [0; 6]), RET_EPERM, "{name} must return EPERM");
        }
        assert_eq!(verdict("setuid", [0, 0, 0, 0, 0, 0]), RET_EPERM, "setuid(0) denied");
        assert_eq!(verdict("setuid", [u64::from(WORKER), 0, 0, 0, 0, 0]), RET_ALLOW);
    }

    #[test]
    fn personality_allows_only_the_benign_personas() {
        for &persona in PERSONALITY_ALLOWED {
            assert_eq!(verdict("personality", [persona, 0, 0, 0, 0, 0]), RET_ALLOW, "{persona:#x}");
        }
        for persona in [0x0004_0000_u64, 0x0040_0000, 0x0004_0008] {
            assert_eq!(
                verdict("personality", [persona, 0, 0, 0, 0, 0]),
                RET_EPERM,
                "{persona:#x} must be denied"
            );
        }
    }

    #[test]
    fn the_agent_trees_everyday_syscalls_stay_allowed() {
        for name in [
            "read",
            "write",
            "mmap",
            "mprotect",
            "clone",
            "clone3",
            "execve",
            "epoll_pwait",
            "openat",
            "futex",
            "landlock_create_ruleset",
        ] {
            assert_eq!(verdict(name, [0; 6]), RET_ALLOW, "{name} must stay allowed");
        }
    }

    /// Runs in the forked child: install the filter, then expect `EPERM` from
    /// calls that would otherwise fail with a different errno or succeed.
    fn probe_in_child(program: &BpfProgram, perf: i64, uring: i64, persona: i64) -> i32 {
        if seccompiler::apply_filter(program).is_err() {
            return 10;
        }
        let eperm = |rc: libc::c_long| {
            rc == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
        };
        // SAFETY: null/zero arguments; the filter answers before the kernel reads them.
        unsafe {
            if !eperm(libc::syscall(perf, 0, 0, -1, -1, 0)) {
                return 11;
            }
            if !eperm(libc::syscall(uring, 1, 0)) {
                return 12;
            }
            if !eperm(libc::syscall(persona, 0x0004_0000)) {
                return 13;
            }
            if libc::syscall(persona, 0xFFFF_FFFF_u32) == -1 {
                return 14;
            }
            if libc::getpid() <= 0 {
                return 15;
            }
        }
        0
    }

    #[test]
    fn installed_filter_denies_new_syscalls_in_the_kernel() {
        let (program, _) = build(WORKER).unwrap();
        let blocked = blocked_syscalls().unwrap();
        let nr = |name: &str| blocked.iter().find(|b| b.name == name).unwrap().num;
        let perf = nr("perf_event_open");
        let uring = nr("io_uring_setup");
        let persona = nr("personality");

        // SAFETY: the child only issues raw syscalls and `_exit`s; it never
        // returns into the test harness.
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0, "fork failed");
        if pid == 0 {
            let code = probe_in_child(&program, perf, uring, persona);
            // SAFETY: terminate the forked child without running destructors.
            unsafe { libc::_exit(code) };
        }
        let mut status = 0;
        // SAFETY: waiting on the child forked above.
        assert_eq!(unsafe { libc::waitpid(pid, &raw mut status, 0) }, pid);
        assert!(libc::WIFEXITED(status), "child did not exit cleanly: {status:#x}");
        assert_eq!(libc::WEXITSTATUS(status), 0, "child step failed (10=apply, 11+=probe)");
    }
}
