//! cctui-supervisor entrypoint.
//!
//! Sequence (order matters):
//!   1. parse + resolve paths
//!   2. with `--guard-exec`: exec user-notification filter, listener to the guard
//!   3. Landlock (best-effort; `--strict` makes a missing LSM fatal)
//!   4. drop capabilities + setgid/setuid  -- BEFORE seccomp, which blocks setuid
//!   5. seccomp denylist (inherited by the exec'd payload, cannot be undone)
//!   6. write report
//!   7. execvp the payload

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
    use cctui_supervisor::{privdrop, seccomp};

    // Hidden self-test hook (used by integration tests, no privileges needed):
    // install the seccomp denylist, then call unshare(0). `unshare` with no
    // flags is a no-op that normally succeeds for any user, so a resulting
    // EPERM is unambiguous proof the filter denied it (a no-op setuid is
    // allowed, so it cannot prove that). Prints the errno and exits with it.
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

    let exec_guarded = arm_exec_guard(&cli)?;
    let landlock_outcome = apply_landlock(&cli, &ro, &rw, exec_guarded)?;

    // 2. Privilege drop (BEFORE seccomp: the denylist blocks setuid/setgid).
    let caps_dropped = if cli.no_privdrop {
        eprintln!("cctui-supervisor: warning: --no-privdrop, running with current uid/caps");
        false
    } else {
        privdrop::drop_capabilities().context("dropping capabilities")?;
        if !privdrop::switch_user(cli.user).context("switching user")? {
            eprintln!("cctui-supervisor: already uid/gid {}, no user switch", cli.user);
        }
        true
    };

    // 3. Seccomp denylist (inherited across exec).
    let (seccomp_applied, blocked) = if cli.no_seccomp {
        eprintln!("cctui-supervisor: warning: --no-seccomp, syscall denylist NOT installed");
        (false, Vec::new())
    } else {
        let names = seccomp::apply(cli.user).context("applying seccomp")?;
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
            exec_guarded,
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

/// Hand the exec user-notification listener to the guard when `--guard-exec`
/// is set. Returns whether exec interception is armed; an error aborts the run.
#[cfg(target_os = "linux")]
fn arm_exec_guard(cli: &Cli) -> anyhow::Result<bool> {
    use anyhow::Context;
    use cctui_supervisor::exec_notify;

    let Some(socket) = &cli.guard_exec else {
        return Ok(false);
    };
    let listener = exec_notify::install().context("installing the exec notification filter")?;
    exec_notify::hand_off(socket, listener, exec_notify::HANDOFF_TIMEOUT)
        .context("handing the exec filter to cctui-guard")?;
    eprintln!("cctui-supervisor: exec interception on (guard at {})", socket.display());
    Ok(true)
}

/// Landlock, best-effort unless `--strict`. Under exec interception the RW paths
/// lose execute rights and an unenforced ruleset is fatal.
#[cfg(target_os = "linux")]
fn apply_landlock(
    cli: &Cli,
    ro: &[String],
    rw: &[String],
    exec_guarded: bool,
) -> anyhow::Result<cctui_supervisor::landlock_rules::Outcome> {
    use anyhow::bail;
    use cctui_supervisor::landlock_rules;

    let outcome = landlock_rules::apply_with(ro, rw, !exec_guarded)?;
    if !outcome.is_enforced() {
        if cli.strict {
            bail!("landlock not enforced and --strict set: {}", outcome.describe());
        }
        if exec_guarded {
            bail!(
                "landlock not enforced and --guard-exec needs it to keep RW paths noexec: {}",
                outcome.describe()
            );
        }
        eprintln!(
            "cctui-supervisor: warning: landlock {} -- continuing (seccomp + cap drop only)",
            outcome.describe()
        );
    }
    Ok(outcome)
}

/// Self-test: install the seccomp denylist then exercise a denied syscall that
/// would otherwise succeed for an unprivileged process. Exits 0 if the kernel
/// returned EPERM (filter works), 2 otherwise.
#[cfg(target_os = "linux")]
fn selftest_seccomp() -> anyhow::Result<()> {
    use anyhow::Context;
    use nix::errno::Errno;
    use nix::sched::{CloneFlags, unshare};

    // Worker id is irrelevant here: `unshare` is denied unconditionally.
    let _ = cctui_supervisor::seccomp::apply(1000).context("self-test: apply seccomp")?;

    // unshare(0) is a no-op that succeeds for any user when allowed; the
    // denylist turns it into EPERM.
    match unshare(CloneFlags::empty()) {
        Err(Errno::EPERM) => {
            eprintln!("self-test: unshare denied with EPERM (filter active)");
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
    if let Some(socket) = &cli.guard_exec {
        println!("Exec interception: cctui-guard at {} (RW paths noexec)", socket.display());
    }
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
