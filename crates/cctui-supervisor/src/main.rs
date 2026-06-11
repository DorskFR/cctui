//! cctui-supervisor entrypoint.
//!
//! Sequence (order matters):
//!   1. parse + resolve paths
//!   2. Landlock (best-effort; `--strict` makes a missing LSM fatal)
//!   3. drop capabilities + setgid/setuid  -- BEFORE seccomp, which blocks setuid
//!   4. seccomp denylist (inherited by the exec'd payload, cannot be undone)
//!   5. write report
//!   6. execvp the payload

use clap::Parser;

use cctui_supervisor::cli::Cli;
use cctui_supervisor::report::Report;

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("cctui-supervisor only supports Linux (Landlock + seccomp)");
    std::process::exit(1);
}

#[cfg(target_os = "linux")]
fn main() {
    if let Err(err) = run() {
        eprintln!("cctui-supervisor: error: {err:#}");
        std::process::exit(1);
    }
}

#[cfg(target_os = "linux")]
fn run() -> anyhow::Result<()> {
    use std::ffi::CString;

    use anyhow::{Context, bail};
    use cctui_supervisor::{landlock_rules, privdrop, seccomp};

    // Hidden self-test hook (used by integration tests, no privileges needed):
    // install the seccomp denylist, then call setuid(geteuid()). Setting your
    // uid to its current value normally succeeds for any user, so a resulting
    // EPERM is unambiguous proof the filter denied it. Prints the errno and
    // exits with that code.
    if std::env::var_os("CCTUI_SUPERVISOR_SELFTEST_SECCOMP").is_some() {
        return selftest_seccomp();
    }

    let cli = Cli::parse();
    let ro = cli.ro_paths();
    let rw = cli.rw_paths();

    if cli.dry_run {
        print_dry_run(&cli, &ro, &rw);
        return Ok(());
    }

    if cli.command.is_empty() {
        bail!("no command provided");
    }

    // 1. Landlock (best-effort).
    let landlock_outcome = landlock_rules::apply(&ro, &rw)?;
    if !landlock_outcome.is_enforced() {
        if cli.strict {
            bail!("landlock not enforced and --strict set: {}", landlock_outcome.describe());
        }
        eprintln!(
            "cctui-supervisor: warning: landlock {} -- continuing (seccomp + cap drop only)",
            landlock_outcome.describe()
        );
    }

    // 2. Privilege drop (BEFORE seccomp: the denylist blocks setuid/setgid).
    let caps_dropped = if cli.no_privdrop {
        eprintln!("cctui-supervisor: warning: --no-privdrop, running with current uid/caps");
        false
    } else {
        privdrop::drop_capabilities().context("dropping capabilities")?;
        privdrop::switch_user(cli.user).context("switching user")?;
        true
    };

    // 3. Seccomp denylist (inherited across exec).
    let (seccomp_applied, blocked) = if cli.no_seccomp {
        eprintln!("cctui-supervisor: warning: --no-seccomp, syscall denylist NOT installed");
        (false, Vec::new())
    } else {
        let names = seccomp::apply().context("applying seccomp")?;
        eprintln!("cctui-supervisor: seccomp blocked {} syscalls: {:?}", names.len(), names);
        (true, names.into_iter().map(ToString::to_string).collect())
    };

    // 4. Report.
    if let Some(path) = &cli.report {
        let report = Report {
            landlock: landlock_outcome.describe(),
            seccomp_applied,
            seccomp_blocked: blocked,
            caps_dropped,
            uid: cli.user,
            ro_paths: ro,
            rw_paths: rw,
            command: cli.command.clone(),
        };
        report.write_to(path)?;
    }

    // 5. execvp the payload (PATH-searched like the Go reference).
    let prog = CString::new(cli.command[0].as_str()).context("command name has interior NUL")?;
    let argv: Vec<CString> = cli
        .command
        .iter()
        .map(|a| CString::new(a.as_str()))
        .collect::<Result<_, _>>()
        .context("command arg has interior NUL")?;

    // execvp only returns on failure.
    let err = nix::unistd::execvp(&prog, &argv).unwrap_err();
    bail!("exec {:?} failed: {}", cli.command[0], err);
}

/// Self-test: install the seccomp denylist then exercise a denied syscall that
/// would otherwise succeed for an unprivileged process. Exits 0 if the kernel
/// returned EPERM (filter works), 2 otherwise.
#[cfg(target_os = "linux")]
fn selftest_seccomp() -> anyhow::Result<()> {
    use anyhow::Context;
    use nix::errno::Errno;
    use nix::unistd::{Uid, geteuid, setuid};

    let _ = cctui_supervisor::seccomp::apply().context("self-test: apply seccomp")?;

    // setuid(geteuid()) is a no-op that succeeds for any user when allowed; the
    // denylist turns it into EPERM.
    let me: Uid = geteuid();
    match setuid(me) {
        Err(Errno::EPERM) => {
            eprintln!("self-test: setuid denied with EPERM (filter active)");
            std::process::exit(0);
        }
        other => {
            eprintln!("self-test: expected EPERM, got {other:?}");
            std::process::exit(2);
        }
    }
}

#[cfg(target_os = "linux")]
fn print_dry_run(cli: &Cli, ro: &[String], rw: &[String]) {
    use cctui_supervisor::syscalls::blocked_syscalls;

    println!("Landlock rules (read-only):");
    for p in ro {
        println!("  {p}");
    }
    println!("Landlock rules (read-write):");
    for p in rw {
        println!("  {p}");
    }
    println!("User (uid/gid): {}", cli.user);
    println!("Strict: {}", cli.strict);
    println!("Seccomp: {}", if cli.no_seccomp { "disabled" } else { "enabled" });
    if !cli.no_seccomp {
        println!("Blocked syscalls:");
        match blocked_syscalls() {
            Some(list) => {
                for b in list {
                    println!("  {} ({})", b.name, b.reason);
                }
            }
            None => println!("  (unsupported architecture)"),
        }
    }
    println!("Command: {:?}", cli.command);
}
