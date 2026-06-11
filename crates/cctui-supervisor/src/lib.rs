//! cctui-supervisor: privilege-drop exec wrapper for the worker payload.
//!
//! Applies Landlock filesystem rules + a seccomp syscall denylist, drops all
//! capabilities and setuids to the worker user, then execs the payload command.
//! Library surface exists so integration tests can drive the pieces (and so the
//! binary stays a thin `main`).

pub mod cli;
pub mod report;

#[cfg(target_os = "linux")]
pub mod landlock_rules;
#[cfg(target_os = "linux")]
pub mod privdrop;
#[cfg(target_os = "linux")]
pub mod seccomp;
#[cfg(target_os = "linux")]
pub mod syscalls;
