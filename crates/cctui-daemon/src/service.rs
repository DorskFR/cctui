//! Per-user service install/uninstall for the daemon.
//!
//! Linux: writes a systemd user unit at
//! `~/.config/systemd/user/cctui-daemon.service` and drives `systemctl
//! --user`.
//!
//! macOS: writes a launchd agent at
//! `~/Library/LaunchAgents/dev.cctui.daemon.plist` and drives
//! `launchctl bootstrap` / `bootout` against the user's GUI session.
//!
//! Other OSes: not yet supported.

use std::process::Command;

use anyhow::{Context, Result, bail};

pub(crate) const UNIT_NAME: &str = "cctui-daemon.service";
pub(crate) const UNIT_TEMPLATE: &str =
    include_str!("../../../packaging/systemd/cctui-daemon.service");
const PLIST_LABEL: &str = "dev.cctui.daemon";
const PLIST_TEMPLATE: &str = include_str!("../../../packaging/launchd/dev.cctui.daemon.plist");

pub fn install() -> Result<()> {
    if cfg!(target_os = "macos") {
        macos::install()
    } else if cfg!(target_os = "linux") {
        linux::install()
    } else {
        bail!("service install: unsupported OS")
    }
}

pub fn uninstall() -> Result<()> {
    if cfg!(target_os = "macos") {
        macos::uninstall()
    } else if cfg!(target_os = "linux") {
        linux::uninstall()
    } else {
        bail!("service uninstall: unsupported OS")
    }
}

/// Restart the daemon service if one is currently running.
///
/// Lets a freshly swapped binary take effect. Returns `Ok(true)` if a managed
/// service was found and restarted, `Ok(false)` if none is active (the caller
/// decides whether to re-exec or print a hint). Errors only if driving the
/// service manager fails outright.
pub fn restart_if_active() -> Result<bool> {
    if cfg!(target_os = "macos") {
        macos::restart_if_active()
    } else if cfg!(target_os = "linux") {
        linux::restart_if_active()
    } else {
        Ok(false)
    }
}

/// Explicitly restart the daemon service (the `service restart` subcommand).
/// Errors if the service manager rejects the request (e.g. no unit installed).
pub fn restart() -> Result<()> {
    if cfg!(target_os = "macos") {
        macos::restart()
    } else if cfg!(target_os = "linux") {
        linux::restart()
    } else {
        bail!("service restart: unsupported OS")
    }
}

/// Whether a managed cctui-daemon service is currently running.
#[must_use]
pub fn is_active() -> bool {
    if cfg!(target_os = "macos") {
        macos::is_active()
    } else if cfg!(target_os = "linux") {
        linux::is_active()
    } else {
        false
    }
}

/// Print the service manager's status plus recent daemon logs to stdout.
///
/// Backs the `service status` subcommand. Output is streamed straight from
/// the underlying tools; a stopped or absent service is reported, not an
/// error.
pub fn status() -> Result<()> {
    if cfg!(target_os = "macos") {
        macos::status();
    } else if cfg!(target_os = "linux") {
        linux::status();
    } else {
        bail!("service status: unsupported OS")
    }
    print_running_version();
    Ok(())
}

/// Report the version of the *running* daemon (from the runtime state file),
/// distinct from `--version` which reflects only this binary. Makes it obvious
/// when a new binary is installed but the service hasn't been restarted onto it.
fn print_running_version() {
    println!("\n--- daemon version ---");
    println!("binary (this CLI): {}", env!("CARGO_PKG_VERSION"));
    match crate::runtime::read() {
        Some(rt) if crate::runtime::pid_alive(rt.pid) => {
            println!("running service:   {} (pid {}, since {})", rt.version, rt.pid, rt.started_at);
        }
        Some(rt) => println!(
            "running service:   none — last run was {} (pid {}, since {}, no longer alive)",
            rt.version, rt.pid, rt.started_at
        ),
        None => println!("running service:   unknown (daemon has not run on this machine)"),
    }
}

/// Print the platform-appropriate unit content (systemd unit on Linux,
/// launchd plist on macOS).
pub fn print_unit() {
    if cfg!(target_os = "macos") {
        print!("{}", macos::rendered_plist().unwrap_or_else(|_| PLIST_TEMPLATE.into()));
    } else {
        print!("{UNIT_TEMPLATE}");
    }
}

fn current_exe_string() -> Result<String> {
    let path = std::env::current_exe().context("locate current executable")?;
    Ok(path.to_string_lossy().into_owned())
}

fn tail_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// `launchctl print` lines worth surfacing on their own (`last exit code =`,
/// `last exit reason =`), so they aren't buried in the full dump.
fn exit_lines(launchctl_print: &str) -> Vec<String> {
    launchctl_print
        .lines()
        .filter(|l| l.trim_start().starts_with("last exit"))
        .map(|l| l.trim().to_owned())
        .collect()
}

/// What `service restart` must do given whether the service manager currently
/// has the job loaded. A restart request against an unloaded launchd job
/// would otherwise silently no-op.
#[derive(Debug, PartialEq, Eq)]
enum RestartPlan {
    Kickstart,
    Install,
}

const fn restart_plan(loaded: bool) -> RestartPlan {
    if loaded { RestartPlan::Kickstart } else { RestartPlan::Install }
}

fn run(cmd: &str, args: &[&str]) -> Result<()> {
    let out = Command::new(cmd)
        .args(args)
        .output()
        .with_context(|| format!("running `{cmd} {}`", args.join(" ")))?;
    if !out.status.success() {
        bail!("`{cmd} {}` failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

mod linux {
    use super::{Context, Result, UNIT_NAME, UNIT_TEMPLATE, run};
    use std::path::{Path, PathBuf};

    fn unit_dir() -> Result<PathBuf> {
        let base = dirs::config_dir().context("no $XDG_CONFIG_HOME / $HOME")?;
        Ok(base.join("systemd").join("user"))
    }

    fn unit_path() -> Result<PathBuf> {
        Ok(unit_dir()?.join(UNIT_NAME))
    }

    fn systemctl(args: &[&str]) -> Result<()> {
        let mut all = vec!["--user"];
        all.extend_from_slice(args);
        run("systemctl", &all)
    }

    pub fn install() -> Result<()> {
        let dir = unit_dir()?;
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let path = dir.join(UNIT_NAME);
        write_unit(&path)?;
        systemctl(&["daemon-reload"])?;
        systemctl(&["enable", "--now", UNIT_NAME])?;
        println!("installed {} and started cctui-daemon.service", path.display());
        Ok(())
    }

    pub fn uninstall() -> Result<()> {
        let _ = systemctl(&["disable", "--now", UNIT_NAME]);
        let path = unit_path()?;
        if path.exists() {
            std::fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
            println!("removed {}", path.display());
        }
        let _ = systemctl(&["daemon-reload"]);
        Ok(())
    }

    /// `is-active --quiet` exits 0 only while the unit is running. Use a raw
    /// status check rather than `run` (which treats non-zero as an error).
    pub fn is_active() -> bool {
        std::process::Command::new("systemctl")
            .args(["--user", "is-active", "--quiet", UNIT_NAME])
            .status()
            .is_ok_and(|s| s.success())
    }

    pub fn restart_if_active() -> Result<bool> {
        if !is_active() {
            return Ok(false);
        }
        systemctl(&["restart", UNIT_NAME])?;
        Ok(true)
    }

    pub fn restart() -> Result<()> {
        systemctl(&["restart", UNIT_NAME])
    }

    /// `systemctl --user status` (tolerating the non-zero exit it returns when
    /// the unit is inactive) followed by the most recent journal lines.
    pub fn status() {
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "status", UNIT_NAME, "--no-pager"])
            .status();
        println!("\n--- recent logs (journalctl --user -t cctui-daemon -n 20) ---");
        let _ = std::process::Command::new("journalctl")
            .args(["--user", "-t", "cctui-daemon", "-n", "20", "--no-pager"])
            .status();
    }

    fn write_unit(path: &Path) -> Result<()> {
        std::fs::write(path, UNIT_TEMPLATE).with_context(|| format!("write {}", path.display()))?;
        Ok(())
    }
}

mod macos {
    use super::{
        Context, PLIST_LABEL, PLIST_TEMPLATE, RestartPlan, Result, bail, current_exe_string,
        restart_plan, run,
    };
    use std::path::PathBuf;
    use std::process::Command;
    use std::time::Duration;

    /// Earlier builds shipped a plist whose `Label` was `com.cctui.daemon`
    /// while the code drove `bootout`/`kickstart` against
    /// `dev.cctui.daemon`. That mismatch left a stale registration that
    /// made the next `bootstrap` fail with `5: Input/output error`. Boot this
    /// out too so upgrades from a broken install heal themselves.
    const LEGACY_LABEL: &str = "com.cctui.daemon";

    fn agents_dir() -> Result<PathBuf> {
        let home = dirs::home_dir().context("no $HOME")?;
        Ok(home.join("Library").join("LaunchAgents"))
    }

    fn plist_path() -> Result<PathBuf> {
        Ok(agents_dir()?.join(format!("{PLIST_LABEL}.plist")))
    }

    fn uid() -> u32 {
        rustix::process::getuid().as_raw()
    }

    /// launchd "gui/<uid>" domain matches the user's logged-in session —
    /// required for an agent that needs to read ~/.claude.
    fn gui_domain() -> String {
        format!("gui/{}", uid())
    }

    fn service_target(label: &str) -> String {
        format!("gui/{}/{label}", uid())
    }

    /// Whether launchd currently knows about a service in our gui domain.
    fn is_loaded(label: &str) -> bool {
        Command::new("launchctl")
            .args(["print", &service_target(label)])
            .output()
            .is_ok_and(|o| o.status.success())
    }

    /// `bootout` is asynchronous — launchd may still report the service as
    /// loaded for a moment after the command returns, and a `bootstrap`
    /// issued into that window fails with `5: Input/output error`. Poll until
    /// the service is gone (bounded), so the subsequent bootstrap is clean.
    fn wait_until_unloaded(label: &str) {
        for _ in 0..30 {
            if !is_loaded(label) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Substitute the embedded plist template with this binary's actual
    /// path. The bundled template hardcodes `/usr/local/bin/cctui-daemon`,
    /// which is wrong if the user installed the binary into
    /// `~/.local/bin` (matching the systemd convention).
    ///
    /// Also render `__CCTUI_DAEMON_PATH__` to the install-time `$PATH`
    /// (augmented with the usual Homebrew/`~/.local/bin` locations). launchd
    /// gives an agent a minimal `PATH` (`/usr/local/bin:/usr/bin:/bin`) that
    /// omits `/opt/homebrew/bin` — so the daemon's `Command::new("codex")` /
    /// `Command::new("claude")` exec'd children fail with ENOENT.
    /// `service install` runs from the user's interactive shell, so its
    /// `$PATH` resolves the tools the daemon will need to spawn.
    pub fn rendered_plist() -> Result<String> {
        let exe = current_exe_string()?;
        let home = std::env::var("HOME").context("HOME not set")?;
        let log_dir = format!("{home}/Library/Logs");
        std::fs::create_dir_all(&log_dir).with_context(|| format!("create {log_dir}"))?;
        Ok(PLIST_TEMPLATE
            .replace("/usr/local/bin/cctui-daemon", &exe)
            .replace("__CCTUI_DAEMON_PATH__", &crate::childenv::child_path())
            .replace("__CCTUI_LOG_DIR__", &log_dir))
    }

    pub fn install() -> Result<()> {
        if uid() == 0 {
            // root has no `gui` domain (no logged-in GUI session), so
            // `bootstrap gui/0 …` fails with `125: Domain does not support
            // specified action`. A user LaunchAgent must load into the real
            // user's session — running under sudo is always wrong here.
            bail!(
                "run `cctui-daemon service install` as your normal user, not with sudo — \
                 this installs a launchd *user agent* into your gui/$UID session; root (uid 0) \
                 has no gui domain"
            );
        }
        let dir = agents_dir()?;
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let path = plist_path()?;
        let content = rendered_plist()?;
        std::fs::write(&path, content).with_context(|| format!("write {}", path.display()))?;

        // Tear down any prior registration — both our label and the legacy
        // one from earlier broken builds — then wait for launchd to actually
        // drop them before bootstrapping.
        let _ = run("launchctl", &["bootout", &service_target(PLIST_LABEL)]);
        let _ = run("launchctl", &["bootout", &service_target(LEGACY_LABEL)]);
        wait_until_unloaded(PLIST_LABEL);
        wait_until_unloaded(LEGACY_LABEL);

        // Clear any lingering "disabled" override so bootstrap isn't rejected.
        let _ = run("launchctl", &["enable", &service_target(PLIST_LABEL)]);

        // If bootstrap still races and the service ends up loaded anyway,
        // tolerate the error rather than failing the install.
        if let Err(e) =
            run("launchctl", &["bootstrap", &gui_domain(), path.to_string_lossy().as_ref()])
            && !is_loaded(PLIST_LABEL)
        {
            return Err(e).context("launchctl bootstrap failed and the agent did not load");
        }
        run("launchctl", &["kickstart", "-k", &service_target(PLIST_LABEL)])?;
        println!("installed {} and started {PLIST_LABEL}", path.display());
        Ok(())
    }

    pub fn uninstall() -> Result<()> {
        let _ = run("launchctl", &["bootout", &service_target(PLIST_LABEL)]);
        let _ = run("launchctl", &["bootout", &service_target(LEGACY_LABEL)]);
        let path = plist_path()?;
        if path.exists() {
            std::fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
            println!("removed {}", path.display());
        }
        Ok(())
    }

    pub fn is_active() -> bool {
        is_loaded(PLIST_LABEL)
    }

    pub fn restart_if_active() -> Result<bool> {
        if !is_loaded(PLIST_LABEL) {
            return Ok(false);
        }
        // `kickstart -k` kills the running instance and starts it fresh.
        run("launchctl", &["kickstart", "-k", &service_target(PLIST_LABEL)])?;
        Ok(true)
    }

    const LOG_TAIL_LINES: usize = 40;

    /// `launchctl print` for the agent (with its `last exit …` lines echoed
    /// prominently), then the tail of the launchd-captured stderr log. The
    /// daemon logs to stderr → `cctui-daemon.err.log`; the unified log
    /// (`log show`) never sees it, so grepping it always came up empty.
    pub fn status() {
        match Command::new("launchctl").args(["print", &service_target(PLIST_LABEL)]).output() {
            Ok(out) => {
                print!("{}", String::from_utf8_lossy(&out.stdout));
                if !out.status.success() {
                    println!(
                        "launchctl print failed ({}): {}",
                        out.status,
                        String::from_utf8_lossy(&out.stderr).trim()
                    );
                }
                let exits = super::exit_lines(&String::from_utf8_lossy(&out.stdout));
                if !exits.is_empty() {
                    println!("\n--- last exit ---");
                    for line in exits {
                        println!("{line}");
                    }
                }
            }
            Err(err) => println!("launchctl print failed: {err}"),
        }
        let Some(home) = dirs::home_dir() else {
            println!("\ncannot locate $HOME — no log paths to read");
            return;
        };
        let logs = home.join("Library").join("Logs");
        let err_log = logs.join("cctui-daemon.err.log");
        println!("\n--- recent logs (last {LOG_TAIL_LINES} lines of {}) ---", err_log.display());
        match std::fs::read_to_string(&err_log) {
            Ok(text) => println!("{}", super::tail_lines(&text, LOG_TAIL_LINES)),
            Err(err) => println!("cannot read {}: {err}", err_log.display()),
        }
        println!("(stdout log: {})", logs.join("cctui-daemon.out.log").display());
    }

    /// `kickstart -k` only restarts a job launchd already knows about; when
    /// the agent is not bootstrapped (fresh machine, or booted out by an
    /// earlier failure) it exits 0 without starting anything, so fall back to
    /// the install path in that case.
    pub fn restart() -> Result<()> {
        match restart_plan(is_loaded(PLIST_LABEL)) {
            RestartPlan::Kickstart => {
                run("launchctl", &["kickstart", "-k", &service_target(PLIST_LABEL)])
            }
            RestartPlan::Install => {
                println!("{PLIST_LABEL} is not loaded in {}; installing it instead", gui_domain());
                install()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_keepalive_only_respawns_on_failure() {
        assert!(PLIST_TEMPLATE.contains("<key>SuccessfulExit</key>"));
        assert!(PLIST_TEMPLATE.contains("<key>ThrottleInterval</key>"));
        assert!(PLIST_TEMPLATE.contains("<integer>30</integer>"));
        let keepalive = PLIST_TEMPLATE.split("<key>KeepAlive</key>").nth(1).unwrap();
        assert!(
            keepalive.trim_start().starts_with("<dict>"),
            "KeepAlive must be the SuccessfulExit dict, not a bare <true/>"
        );
    }

    #[test]
    fn restart_plan_installs_when_not_loaded_and_kickstarts_when_loaded() {
        assert_eq!(restart_plan(false), RestartPlan::Install);
        assert_eq!(restart_plan(true), RestartPlan::Kickstart);
    }

    #[test]
    fn tail_lines_keeps_only_the_last_n() {
        let text = (1..=10).map(|i| format!("line{i}")).collect::<Vec<_>>().join("\n");
        assert_eq!(tail_lines(&text, 3), "line8\nline9\nline10");
        assert_eq!(tail_lines(&text, 100), text);
        assert_eq!(tail_lines("", 5), "");
    }

    #[test]
    fn exit_lines_extracts_launchctl_exit_fields() {
        let print = "\tstate = not running\n\tlast exit code = 78\n\tlast exit reason = exited\n\tpid = 0\n";
        assert_eq!(exit_lines(print), vec!["last exit code = 78", "last exit reason = exited"]);
        assert!(exit_lines("state = running").is_empty());
    }
}
