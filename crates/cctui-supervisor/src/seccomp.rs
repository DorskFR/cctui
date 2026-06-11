//! Seccomp-BPF denylist.
//!
//! Port of `seccomp.go`: builds a filter that returns `EPERM` for a fixed set
//! of dangerous syscalls and allows everything else. The Go original hand-rolls
//! the BPF program; here `seccompiler` emits an equivalent program (arch guard,
//! per-syscall comparisons, default allow). `seccompiler::apply_filter` also
//! sets `PR_SET_NO_NEW_PRIVS` before installing the filter, matching the Go
//! `prctl(PR_SET_NO_NEW_PRIVS)` call.

use anyhow::{Context, Result, anyhow};
use seccompiler::{BpfProgram, SeccompAction, SeccompFilter, TargetArch};

use crate::syscalls::blocked_syscalls;

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

/// Build the denylist BPF program.
///
/// Each blocked syscall returns `EPERM`; every other syscall is allowed (the
/// mismatch action). Returns the program and the ordered list of blocked
/// syscall names (for logging / the report).
pub fn build() -> Result<(BpfProgram, Vec<&'static str>)> {
    let arch = target_arch()?;
    let blocked =
        blocked_syscalls().ok_or_else(|| anyhow!("no syscall number table for this arch"))?;

    // Empty rule vec on a syscall => match unconditionally, take match_action.
    let rules = blocked.iter().map(|b| (b.num, vec![])).collect();

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

/// Build and install the denylist on the calling thread. Returns the blocked
/// syscall names. `apply_filter` sets `NO_NEW_PRIVS` internally.
pub fn apply() -> Result<Vec<&'static str>> {
    let (program, names) = build()?;
    seccompiler::apply_filter(&program).context("installing seccomp filter")?;
    Ok(names)
}
