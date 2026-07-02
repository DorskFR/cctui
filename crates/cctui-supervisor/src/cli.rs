//! CLI surface. Port of the Go `flag` parsing, extended with the configurable
//! path sets, `--user`, `--report` and `--strict` the ticket calls for.

use std::path::PathBuf;

use clap::Parser;

/// Default read-only paths (neutral worker-contract paths — PUBLIC repo, no
/// homelab-specific paths). Applied when no `--ro` flag is given.
///
/// `/proc` is required: language runtimes (node/V8 behind `claude`, and most
/// interpreters) read `/proc/self/*`, cpuinfo, etc. during early init, and a
/// Landlock denial there makes them `abort()` silently before any output.
///
/// This set is fixed here, but a DERIVED worker image whose toolchain lives
/// outside it (e.g. Node/pnpm under `/opt/mise`, Rust under `/opt/rust`) does not
/// need to fork the entrypoint: `deploy/worker-entrypoint.sh` reads the
/// colon-separated `CCTUI_WORKER_EXTRA_RO` env var and appends a `--ro <path>`
/// for each entry to the supervisor invocation, extending the RO set at boot
/// (CCT-528).
pub const DEFAULT_RO: &[&str] =
    &["/usr", "/lib", "/lib64", "/bin", "/sbin", "/etc", "/proc", "/prompts", "/opt/context"];

/// Default read-write paths (neutral worker-contract paths). Applied when no
/// `--rw` flag is given.
pub const DEFAULT_RW: &[&str] = &[
    "/dev",
    "/tmp",
    "/workspace",
    "/home/worker",
    "/var/run/workflow-guard",
    "/var/run/guard-proxy",
];

/// Default uid the payload is dropped to (the worker user).
pub const DEFAULT_UID: u32 = 1000;

#[derive(Parser, Debug)]
// The debug-toggle flags are genuinely independent booleans.
#[allow(clippy::struct_excessive_bools)]
#[command(
    name = "cctui-supervisor",
    about = "Privilege-drop exec wrapper: applies Landlock + a seccomp denylist, \
             drops capabilities, setuids to the worker user, then execs the payload.",
    version
)]
pub struct Cli {
    /// Read-only path (repeatable). Overrides the default RO set entirely.
    #[arg(long = "ro")]
    pub ro: Vec<String>,

    /// Read-write path (repeatable). Overrides the default RW set entirely.
    #[arg(long = "rw")]
    pub rw: Vec<String>,

    /// Uid (and primary gid) to drop to before exec.
    #[arg(long, default_value_t = DEFAULT_UID)]
    pub user: u32,

    /// Write a JSON hardening report to this path.
    #[arg(long)]
    pub report: Option<PathBuf>,

    /// Treat a missing/unenforced Landlock LSM as fatal instead of warning and
    /// continuing with seccomp + capability drop.
    #[arg(long)]
    pub strict: bool,

    /// Skip the seccomp filter (debugging only).
    #[arg(long = "no-seccomp")]
    pub no_seccomp: bool,

    /// Skip the capability drop / setuid (debugging only).
    #[arg(long = "no-privdrop")]
    pub no_privdrop: bool,

    /// Print the resolved rules and exit without applying anything.
    #[arg(long = "dry-run")]
    pub dry_run: bool,

    /// The command to exec, and its arguments (after `--`).
    #[arg(trailing_var_arg = true, required_unless_present = "dry_run")]
    pub command: Vec<String>,
}

impl Cli {
    /// Resolved read-only set: provided `--ro` flags, or the neutral defaults.
    #[must_use]
    pub fn ro_paths(&self) -> Vec<String> {
        if self.ro.is_empty() {
            DEFAULT_RO.iter().map(ToString::to_string).collect()
        } else {
            self.ro.clone()
        }
    }

    /// Resolved read-write set: provided `--rw` flags, or the neutral defaults.
    #[must_use]
    pub fn rw_paths(&self) -> Vec<String> {
        if self.rw.is_empty() {
            DEFAULT_RW.iter().map(ToString::to_string).collect()
        } else {
            self.rw.clone()
        }
    }
}
