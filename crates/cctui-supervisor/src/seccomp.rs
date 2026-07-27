//! Seccomp-BPF denylist.
//!
//! Port of `seccomp.go`: builds a filter that returns `EPERM` for a fixed set
//! of dangerous syscalls and allows everything else. The Go original hand-rolls
//! the BPF program; here `seccompiler` emits an equivalent program (arch guard,
//! per-syscall comparisons, default allow). `seccompiler::apply_filter` also
//! sets `PR_SET_NO_NEW_PRIVS` before installing the filter, matching the Go
//! `prctl(PR_SET_NO_NEW_PRIVS)` call.

use anyhow::{Context, Result, anyhow};
use seccompiler::{
    BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
    SeccompRule, TargetArch,
};

use crate::syscalls::{blocked_syscalls, id_setter_argc};

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
        let syscall_rules = match id_setter_argc(b.name) {
            Some(argc) => id_setter_rules(argc, u64::from(worker_id))?,
            None => vec![],
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
