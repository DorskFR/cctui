//! CLI surface.

use std::ffi::OsStr;
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
/// for each entry to the supervisor invocation, extending the RO set at boot.
/// Landlock matches inode hierarchies, so `/var/run/guard-proxy-ca` needs its
/// own entry — the sibling `/var/run/guard-proxy` does not cover it.
pub const DEFAULT_RO: &[&str] = &[
    "/usr",
    "/lib",
    "/lib64",
    "/bin",
    "/sbin",
    "/etc",
    "/proc",
    "/prompts",
    "/opt/context",
    "/var/run/guard-proxy-ca",
];

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

/// Operator opt-out of strict Landlock: only the exact value `0` makes a
/// missing Landlock LSM non-fatal. Reserved: a dispatch env cannot set it.
pub const LANDLOCK_STRICT_ENV: &str = "CCTUI_LANDLOCK_STRICT";

/// Whether an unenforced Landlock ruleset is fatal. Strict unless
/// `CCTUI_LANDLOCK_STRICT=0`; `--strict` overrides the opt-out.
#[must_use]
pub fn landlock_strict(flag: bool, env: Option<&OsStr>) -> bool {
    flag || env.is_none_or(|v| v != OsStr::new("0"))
}

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

    /// Treat a missing/unenforced Landlock LSM as fatal. This is the default;
    /// the flag additionally overrides a `CCTUI_LANDLOCK_STRICT=0` opt-out.
    #[arg(long)]
    pub strict: bool,

    /// Skip the seccomp filter (debugging only).
    #[arg(long = "no-seccomp")]
    pub no_seccomp: bool,

    /// Skip the capability drop / setuid (debugging only).
    #[arg(long = "no-privdrop")]
    pub no_privdrop: bool,

    /// Route every execve/execveat of the payload tree through cctui-guard: install
    /// a seccomp user-notification filter, hand its listener fd to the guard
    /// listening on this unix socket, and drop execute rights on the RW paths.
    /// Any failure aborts before the payload runs.
    #[arg(long = "guard-exec", value_name = "SOCKET")]
    pub guard_exec: Option<PathBuf>,

    /// Print the resolved rules and exit without applying anything.
    #[arg(long = "dry-run")]
    pub dry_run: bool,

    /// The command to exec, and its arguments (after `--`).
    #[arg(trailing_var_arg = true, required_unless_present = "dry_run")]
    pub command: Vec<String>,
}

impl Cli {
    /// Effective Landlock strictness for this process (see [`landlock_strict`]).
    #[must_use]
    pub fn landlock_strict(&self) -> bool {
        landlock_strict(self.strict, std::env::var_os(LANDLOCK_STRICT_ENV).as_deref())
    }

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

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::path::Path;

    use clap::Parser;

    use super::{Cli, landlock_strict};

    #[test]
    fn landlock_is_strict_by_default() {
        let cli = Cli::try_parse_from(["cctui-supervisor", "--", "cctui-daemon"]).unwrap();
        assert!(!cli.strict, "no flag given");
        assert!(landlock_strict(cli.strict, None), "strict without flag or env");
    }

    #[test]
    fn only_an_exact_zero_opts_out_of_strict_landlock() {
        assert!(!landlock_strict(false, Some(OsStr::new("0"))));
        for v in ["1", "", "false", "off", "no", "00", " 0", "0 "] {
            assert!(landlock_strict(false, Some(OsStr::new(v))), "{v:?} must stay strict");
        }
    }

    #[test]
    fn strict_flag_overrides_the_opt_out() {
        let cli =
            Cli::try_parse_from(["cctui-supervisor", "--strict", "--", "cctui-daemon"]).unwrap();
        assert!(cli.strict);
        assert!(landlock_strict(cli.strict, Some(OsStr::new("0"))));
    }

    #[test]
    fn there_is_no_cli_flag_to_turn_strict_off() {
        for flag in ["--no-strict", "--strict=false", "--lax"] {
            assert!(
                Cli::try_parse_from(["cctui-supervisor", flag, "--", "cctui-daemon"]).is_err(),
                "{flag} must not parse"
            );
        }
    }

    #[test]
    fn guard_exec_is_off_by_default() {
        let cli = Cli::try_parse_from(["cctui-supervisor", "--", "cctui-daemon", "run"]).unwrap();
        assert_eq!(cli.guard_exec, None);
        assert_eq!(cli.command, ["cctui-daemon", "run"]);
    }

    #[test]
    fn guard_exec_takes_the_socket_path() {
        let cli = Cli::try_parse_from([
            "cctui-supervisor",
            "--guard-exec",
            "/var/run/workflow-guard/exec.sock",
            "--",
            "cctui-daemon",
        ])
        .unwrap();
        assert_eq!(cli.guard_exec.as_deref(), Some(Path::new("/var/run/workflow-guard/exec.sock")));
        assert_eq!(cli.command, ["cctui-daemon"]);
    }
}
