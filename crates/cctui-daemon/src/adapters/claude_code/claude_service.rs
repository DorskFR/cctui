//! Install & run the on-demand `claude daemon` under the OS user service
//! manager, decoupling its lifetime from cctui-daemon.
//!
//! The service manager parents the claude daemon: it supervises, restarts and
//! always reaps it, so cctui-daemon never parents the process (nothing to
//! zombie, no lifecycle coupling) and only ever *connects* to `control.sock`.
//!
//! The supervisor is kept ALWAYS RESIDENT (launchd `KeepAlive` / systemd
//! `Restart=on-failure`) rather than idle-shutting-down: the socket is then
//! always present, which removes the kickstart race (deliberate
//! behavior change).
//!
//! CRITICAL (Linux): this must be its OWN systemd **user** unit, NOT part of
//! `cctui-daemon.service` — that unit runs `KillMode=control-group`, so
//! sharing its cgroup would make a cctui-daemon restart SIGTERM the claude
//! supervisor too (the coupling removes). A separate unit isolates it.

use anyhow::Result;

/// Placeholder in the bundled templates for the resolved `claude` binary path.
const BIN_PLACEHOLDER: &str = "__CLAUDE_BIN__";
/// Placeholder in the bundled templates for the augmented child `PATH`.
const PATH_PLACEHOLDER: &str = "__CLAUDE_DAEMON_PATH__";

const UNIT_NAME: &str = "claude-daemon.service";
const UNIT_TEMPLATE: &str = include_str!("../../../../../packaging/systemd/claude-daemon.service");
#[cfg(target_os = "macos")]
const PLIST_LABEL: &str = "dev.claude.daemon";
#[cfg(any(target_os = "macos", test))]
const PLIST_TEMPLATE: &str =
    include_str!("../../../../../packaging/launchd/dev.claude.daemon.plist");

/// Ensure a claude daemon **supervisor is running**. Callers only reach this
/// once `locate_live()` came back empty, so unit state answers nothing: with
/// `ExitType=cgroup` the unit stays `active` for as long as any adopted worker
/// lives, which is exactly the case where no supervisor is left to serve the
/// socket. [`ensure_supervisor`] holds the decision. A stale unit is rewritten
/// and daemon-reloaded in place, never restarted, so live session jobs survive.
/// Best-effort — the caller falls back to a direct spawn.
pub(super) fn ensure(claude_bin: &str) -> Result<()> {
    if !manager_usable() {
        anyhow::bail!("{BLOCKED}");
    }
    os::ensure(claude_bin)
}

/// Bring an already-installed managed unit up to the bundled template, e.g.
/// after a cctui-daemon self-update ships a changed unit. [`ensure`] only runs
/// when the control socket is missing, so a machine whose supervisor stays up
/// would otherwise keep a stale unit indefinitely. Rewrite + daemon-reload
/// only: never installs, starts or restarts anything, so live session jobs are
/// untouched. Returns whether the unit was rewritten. Linux only; the launchd
/// plist is only (re)written when the agent is not loaded.
pub(super) fn refresh_installed(claude_bin: &str) -> Result<bool> {
    if !manager_usable() {
        return Ok(false);
    }
    os::refresh_installed(claude_bin)
}

/// Whether the managed service is currently the thing running the daemon. A
/// daemon started some other way (`origin: foreground`) survives a unit
/// restart untouched, so the caller must pick a different remedy.
pub(super) fn service_active() -> bool {
    manager_usable() && os::service_active()
}

/// Restart the managed claude-daemon service. Callers must have established
/// that no worker is running: this tears the supervisor down.
pub(super) fn restart(claude_bin: &str) -> Result<()> {
    if !manager_usable() {
        anyhow::bail!("{BLOCKED}");
    }
    os::restart(claude_bin)
}

const BLOCKED: &str = "the OS user service manager is not usable in this build";

/// The service-manager operations [`ensure_supervisor`] needs. Injected so the
/// decision is exercised without a real `systemctl` anywhere near it.
///
/// Only systemd hits the `ExitType=cgroup` trap; launchd tracks the real
/// process, so the macOS path does not implement this.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(super) trait ServiceManager {
    /// Bring the installed unit up to the bundled template. No start, no restart.
    fn sync_unit(&self) -> Result<()>;
    fn is_active(&self) -> bool;
    /// The unit's live main process. `None` once systemd lost it — which
    /// `ExitType=cgroup` makes routine after the claude CLI's upgrade
    /// self-restart, and is indistinguishable from "the supervisor died".
    fn main_pid(&self) -> Option<u32>;
    /// `enable --now` an inactive unit.
    fn start_unit(&self) -> Result<()>;
    /// Launch a supervisor *into the active unit's cgroup*, without restarting
    /// it: a restart would `KillMode=control-group` the adopted workers.
    fn adopt_supervisor(&self) -> Result<()>;
}

#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(super) enum Ensured {
    UnitStarted,
    /// A supervisor process is up; it simply has not bound the socket yet.
    SupervisorStarting,
    Adopted,
}

/// Decide how to get a supervisor back, knowing the caller saw no socket.
///
/// An active unit with no main process is the `ExitType=cgroup` trap: systemd
/// will never fire `Restart=` (the cgroup is not empty, the unit never fails),
/// `start` is a no-op on an active unit, and `restart` kills every live
/// session. Adoption into the existing cgroup is the only move that both
/// launches a supervisor now and leaves the workers alone.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn ensure_supervisor(mgr: &impl ServiceManager) -> Result<Ensured> {
    mgr.sync_unit()?;
    if !mgr.is_active() {
        mgr.start_unit()?;
        return Ok(Ensured::UnitStarted);
    }
    if mgr.main_pid().is_some() {
        return Ok(Ensured::SupervisorStarting);
    }
    mgr.adopt_supervisor()?;
    Ok(Ensured::Adopted)
}

/// A test build must never write the developer's real systemd unit or launchd
/// plist, nor run `systemctl --user` / `launchctl`: every entry point here
/// routes through this gate.
const fn manager_usable() -> bool {
    !cfg!(test)
}

/// Whether an OS user service manager is usable here. Worker containers have
/// no systemd (`/run/systemd/system` absent, no user bus for `systemctl
/// --user`): the kickstarter must then spawn `claude daemon run` as
/// a direct child instead of calling [`ensure`].
pub(super) fn manager_available() -> bool {
    manager_usable() && os::manager_available()
}

/// Resolve `claude_bin` to an absolute path. Service-manager `ExecStart` /
/// `ProgramArguments` require an absolute program path, but the configured
/// `claude_bin` is frequently the bare name `"claude"`. Search the augmented
/// child `PATH` (the same one the service will run with) for it; fall back to
/// the input unchanged so a bad config surfaces as a start failure, not a
/// silent no-op.
fn resolve_claude_bin(claude_bin: &str) -> String {
    if claude_bin.contains('/') {
        return claude_bin.to_string();
    }
    for dir in crate::childenv::child_path().split(':') {
        if dir.is_empty() {
            continue;
        }
        let cand = std::path::Path::new(dir).join(claude_bin);
        if cand.is_file() {
            return cand.to_string_lossy().into_owned();
        }
    }
    claude_bin.to_string()
}

/// Render the systemd user unit for the given claude binary.
fn render_unit(claude_bin: &str) -> String {
    UNIT_TEMPLATE
        .replace(BIN_PLACEHOLDER, &resolve_claude_bin(claude_bin))
        .replace(PATH_PLACEHOLDER, &crate::childenv::child_path())
}

/// Render the launchd `LaunchAgent` plist for the given claude binary.
#[cfg(any(target_os = "macos", test))]
fn render_plist(claude_bin: &str) -> String {
    PLIST_TEMPLATE
        .replace(BIN_PLACEHOLDER, &resolve_claude_bin(claude_bin))
        .replace(PATH_PLACEHOLDER, &crate::childenv::child_path())
}

#[cfg(target_os = "linux")]
use linux as os;
#[cfg(target_os = "macos")]
use macos as os;
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
use unsupported as os;

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod unsupported {
    use anyhow::{Result, bail};

    pub(super) fn ensure(_claude_bin: &str) -> Result<()> {
        bail!("claude daemon service: unsupported OS")
    }
    pub(super) fn refresh_installed(_claude_bin: &str) -> Result<bool> {
        Ok(false)
    }
    pub(super) const fn service_active() -> bool {
        false
    }
    pub(super) fn restart(_claude_bin: &str) -> Result<()> {
        bail!("claude daemon service: unsupported OS")
    }
    pub(super) const fn manager_available() -> bool {
        false
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::{UNIT_NAME, render_unit};
    use anyhow::{Context, Result, bail};
    use std::path::PathBuf;
    use std::process::Command;

    fn unit_dir() -> Result<PathBuf> {
        let base = dirs::config_dir().context("no $XDG_CONFIG_HOME / $HOME")?;
        Ok(base.join("systemd").join("user"))
    }

    pub(super) fn manager_available() -> bool {
        if std::env::var_os("SYSTEMD_OFFLINE").is_some_and(|v| v == "1") {
            return false;
        }
        std::path::Path::new("/run/systemd/system").is_dir()
    }

    pub(super) fn service_active() -> bool {
        is_active()
    }

    fn is_active() -> bool {
        Command::new("systemctl")
            .args(["--user", "is-active", "--quiet", UNIT_NAME])
            .status()
            .is_ok_and(|s| s.success())
    }

    fn systemctl(args: &[&str]) -> Result<()> {
        let mut all = vec!["--user"];
        all.extend_from_slice(args);
        let out = Command::new("systemctl")
            .args(&all)
            .output()
            .with_context(|| format!("running `systemctl {}`", all.join(" ")))?;
        if !out.status.success() {
            bail!(
                "`systemctl {}` failed: {}",
                all.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(())
    }

    /// The real `systemctl --user` behind [`super::ServiceManager`]. Only ever
    /// constructed from [`ensure`], which the `manager_usable` gate keeps out
    /// of test builds.
    struct Systemctl<'a> {
        claude_bin: &'a str,
    }

    impl super::ServiceManager for Systemctl<'_> {
        fn sync_unit(&self) -> Result<()> {
            let dir = unit_dir()?;
            let path = dir.join(UNIT_NAME);
            let rendered = render_unit(self.claude_bin);
            if std::fs::read_to_string(&path).is_ok_and(|cur| cur == rendered) {
                return Ok(());
            }
            std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
            write_and_reload(&path, &rendered)
        }

        fn is_active(&self) -> bool {
            is_active()
        }

        fn main_pid(&self) -> Option<u32> {
            let pid: u32 = show("MainPID")?.parse().ok()?;
            (pid != 0 && std::path::Path::new(&format!("/proc/{pid}")).exists()).then_some(pid)
        }

        fn start_unit(&self) -> Result<()> {
            // Its own user unit -> its own cgroup, never cctui-daemon.service's
            // KillMode=control-group cgroup.
            systemctl(&["enable", "--now", UNIT_NAME])?;
            tracing::info!(unit = UNIT_NAME, "installed and started managed claude daemon");
            Ok(())
        }

        fn adopt_supervisor(&self) -> Result<()> {
            let cgroup = show("ControlGroup")
                .filter(|c| c.starts_with('/'))
                .with_context(|| format!("{UNIT_NAME} is active but reports no control group"))?;
            let procs = std::path::Path::new("/sys/fs/cgroup")
                .join(cgroup.trim_start_matches('/'))
                .join("cgroup.procs");
            let mut child = spawn_supervisor(self.claude_bin)?;
            let pid = child.id();
            // A supervisor in the wrong cgroup still serves the socket, so a
            // failed move is a warning, not a failure.
            match std::fs::write(&procs, pid.to_string()) {
                Ok(()) => tracing::info!(pid, unit = UNIT_NAME, "adopted a new claude supervisor"),
                Err(err) => tracing::warn!(
                    %err, pid, procs = %procs.display(),
                    "claude supervisor started but could not be moved into the unit cgroup"
                ),
            }
            // We remain its parent whatever cgroup it sits in, so it still
            // has to be reaped.
            std::thread::spawn(move || {
                let status = child.wait();
                tracing::info!(pid, ?status, "adopted claude supervisor exited");
            });
            Ok(())
        }
    }

    /// `systemctl --user show -p <prop> --value`, trimmed; `None` when empty.
    fn show(prop: &str) -> Option<String> {
        let out = Command::new("systemctl")
            .args(["--user", "show", "-p", prop, "--value", UNIT_NAME])
            .output()
            .ok()?;
        let v = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        (out.status.success() && !v.is_empty()).then_some(v)
    }

    /// Detached `claude daemon run`, reaped by whoever ends up its parent.
    fn spawn_supervisor(claude_bin: &str) -> Result<std::process::Child> {
        use crate::childenv::ScrubChildEnv as _;
        use std::process::Stdio;

        let mut cmd = Command::new(claude_bin);
        cmd.args(["daemon", "run"])
            .env("PATH", crate::childenv::child_path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        cmd.scrub_child_env();
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            cmd.process_group(0);
        }
        cmd.spawn().with_context(|| format!("spawning `{claude_bin} daemon run`"))
    }

    pub(super) fn ensure(claude_bin: &str) -> Result<()> {
        let outcome = super::ensure_supervisor(&Systemctl { claude_bin })?;
        tracing::debug!(?outcome, unit = UNIT_NAME, "claude supervisor ensured");
        Ok(())
    }

    /// Rewrite the unit and daemon-reload, never restart: live session jobs
    /// must survive. A running service picks up the new directives on reload.
    fn write_and_reload(path: &std::path::Path, rendered: &str) -> Result<()> {
        std::fs::write(path, rendered).with_context(|| format!("write {}", path.display()))?;
        systemctl(&["daemon-reload"])?;
        tracing::info!(unit = UNIT_NAME, "refreshed managed claude daemon unit");
        Ok(())
    }

    pub(super) fn refresh_installed(claude_bin: &str) -> Result<bool> {
        let path = unit_dir()?.join(UNIT_NAME);
        // Not installed: nothing to refresh, `ensure` installs it on demand.
        let Ok(cur) = std::fs::read_to_string(&path) else {
            return Ok(false);
        };
        let rendered = render_unit(claude_bin);
        if cur == rendered {
            return Ok(false);
        }
        write_and_reload(&path, &rendered)?;
        Ok(true)
    }

    pub(super) fn restart(claude_bin: &str) -> Result<()> {
        use super::ServiceManager as _;

        let mgr = Systemctl { claude_bin };
        mgr.sync_unit()?;
        if !mgr.is_active() {
            return mgr.start_unit();
        }
        systemctl(&["restart", UNIT_NAME])?;
        tracing::info!(unit = UNIT_NAME, "restarted managed claude daemon");
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::{PLIST_LABEL, render_plist};
    use anyhow::{Context, Result, bail};
    use std::path::PathBuf;
    use std::process::Command;

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

    fn gui_domain() -> String {
        format!("gui/{}", uid())
    }

    fn service_target() -> String {
        format!("gui/{}/{PLIST_LABEL}", uid())
    }

    pub(super) const fn manager_available() -> bool {
        true
    }

    pub(super) fn refresh_installed(_claude_bin: &str) -> Result<bool> {
        Ok(false)
    }

    pub(super) fn service_active() -> bool {
        is_loaded()
    }

    fn is_loaded() -> bool {
        Command::new("launchctl")
            .args(["print", &service_target()])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn run(cmd: &str, args: &[&str]) -> Result<()> {
        let out = Command::new(cmd)
            .args(args)
            .output()
            .with_context(|| format!("running `{cmd} {}`", args.join(" ")))?;
        if !out.status.success() {
            bail!(
                "`{cmd} {}` failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(())
    }

    pub(super) fn ensure(claude_bin: &str) -> Result<()> {
        // Already loaded: KeepAlive keeps it resident, nothing to do. Skipping
        // avoids a bootout/bootstrap that would needlessly restart the live
        // supervisor on every kickstart poll.
        if is_loaded() {
            return Ok(());
        }
        if uid() == 0 {
            bail!(
                "the managed claude daemon is a launchd *user agent* — it must load into \
                 gui/$UID; root (uid 0) has no gui domain"
            );
        }
        let dir = agents_dir()?;
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let path = plist_path()?;
        std::fs::write(&path, render_plist(claude_bin))
            .with_context(|| format!("write {}", path.display()))?;

        let _ = run("launchctl", &["enable", &service_target()]);
        // RunAtLoad + KeepAlive start the supervisor as soon as it bootstraps.
        if let Err(e) =
            run("launchctl", &["bootstrap", &gui_domain(), path.to_string_lossy().as_ref()])
            && !is_loaded()
        {
            return Err(e).context("launchctl bootstrap of the claude daemon agent failed");
        }
        tracing::info!(label = PLIST_LABEL, "installed and started managed claude daemon");
        Ok(())
    }

    pub(super) fn restart(claude_bin: &str) -> Result<()> {
        ensure(claude_bin)?;
        run("launchctl", &["kickstart", "-k", &service_target()])?;
        tracing::info!(label = PLIST_LABEL, "restarted managed claude daemon");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[derive(Default)]
    struct FakeManager {
        active: bool,
        main_pid: Option<u32>,
        synced: Cell<bool>,
        started: Cell<bool>,
        adopted: Cell<bool>,
    }

    impl ServiceManager for FakeManager {
        fn sync_unit(&self) -> Result<()> {
            self.synced.set(true);
            Ok(())
        }
        fn is_active(&self) -> bool {
            self.active
        }
        fn main_pid(&self) -> Option<u32> {
            self.main_pid
        }
        fn start_unit(&self) -> Result<()> {
            self.started.set(true);
            Ok(())
        }
        fn adopt_supervisor(&self) -> Result<()> {
            self.adopted.set(true);
            Ok(())
        }
    }

    /// The `ExitType=cgroup` trap: the unit is active only because adopted
    /// workers still live in its cgroup, and no supervisor answers. Nothing
    /// systemd does on its own can fix that, so a supervisor must be launched.
    #[test]
    fn an_active_unit_with_no_supervisor_gets_one_launched_into_its_cgroup() {
        let mgr = FakeManager { active: true, main_pid: None, ..FakeManager::default() };
        assert_eq!(ensure_supervisor(&mgr).unwrap(), Ensured::Adopted);
        assert!(mgr.adopted.get(), "a supervisor must be launched");
        assert!(!mgr.started.get(), "`start` is a no-op on an active unit");
        assert!(mgr.synced.get(), "the unit is brought up to the template first");
    }

    #[test]
    fn an_inactive_unit_is_started_through_the_service_manager() {
        let mgr = FakeManager { active: false, main_pid: None, ..FakeManager::default() };
        assert_eq!(ensure_supervisor(&mgr).unwrap(), Ensured::UnitStarted);
        assert!(mgr.started.get());
        assert!(!mgr.adopted.get(), "nothing to adopt: the unit owns its own cgroup");
    }

    /// A live main process means a supervisor exists and is merely slow to
    /// bind; a second one would race it for the socket.
    #[test]
    fn a_live_main_process_is_left_alone() {
        let mgr = FakeManager { active: true, main_pid: Some(4242), ..FakeManager::default() };
        assert_eq!(ensure_supervisor(&mgr).unwrap(), Ensured::SupervisorStarting);
        assert!(!mgr.adopted.get());
        assert!(!mgr.started.get());
    }

    #[test]
    fn unit_runs_claude_daemon_run_with_augmented_path() {
        let unit = render_unit("/opt/homebrew/bin/claude");
        assert!(
            unit.contains("ExecStart=/opt/homebrew/bin/claude daemon run"),
            "unit must exec `claude daemon run`:\n{unit}"
        );
        // PATH placeholder is rendered to the augmented child PATH.
        assert!(!unit.contains(PATH_PLACEHOLDER), "PATH placeholder not substituted:\n{unit}");
        assert!(unit.contains("Environment=PATH="), "unit must set an explicit PATH:\n{unit}");
        for want in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"] {
            assert!(unit.contains(want), "augmented PATH must contain {want}:\n{unit}");
        }
    }

    #[test]
    fn unit_is_restart_on_failure_and_out_of_cctui_cgroup() {
        let unit = render_unit("/usr/local/bin/claude");
        assert!(unit.contains("Restart=on-failure"), "always-resident supervisor:\n{unit}");
        assert!(
            unit.contains("OOMPolicy=continue"),
            "an OOM-killed child must not fail the unit and kill every session:\n{unit}"
        );
        assert!(
            unit.lines().any(|l| l.trim() == "ExitType=cgroup"),
            "the upgrade self-restart must not deactivate the unit and kill every session:\n{unit}"
        );
        // Its own unit — must NOT reference cctui-daemon's unit/cgroup, or a
        // cctui restart (KillMode=control-group) would take it down with it.
        let directives: String = unit
            .lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !directives.contains("cctui-daemon.service"),
            "must not co-locate with cctui:\n{unit}"
        );
        assert!(
            !directives.to_lowercase().contains("killmode"),
            "must not adopt cctui KillMode:\n{unit}"
        );
    }

    #[test]
    fn plist_runs_claude_daemon_run_with_keepalive_and_path() {
        let plist = render_plist("/opt/homebrew/bin/claude");
        assert!(plist.contains("<string>/opt/homebrew/bin/claude</string>"));
        assert!(plist.contains("<string>daemon</string>"));
        assert!(plist.contains("<string>run</string>"));
        // Always resident: RunAtLoad + KeepAlive.
        assert!(plist.contains("<key>RunAtLoad</key>"));
        assert!(plist.contains("<key>KeepAlive</key>"));
        // Augmented PATH baked in (launchd minimal-PATH fix).
        assert!(!plist.contains(PATH_PLACEHOLDER), "PATH placeholder not substituted:\n{plist}");
        assert!(plist.contains("/opt/homebrew/bin"), "augmented PATH:\n{plist}");
        // Its own label — never the cctui daemon's.
        assert!(plist.contains("<string>dev.claude.daemon</string>"));
        assert!(!plist.contains("dev.cctui.daemon"), "must not reuse cctui label:\n{plist}");
    }

    #[test]
    fn resolve_keeps_absolute_paths() {
        assert_eq!(resolve_claude_bin("/opt/homebrew/bin/claude"), "/opt/homebrew/bin/claude");
    }

    #[test]
    fn test_builds_never_reach_the_real_user_service_manager() {
        assert!(!manager_usable());
        assert!(!manager_available());
        assert!(!service_active());
        assert!(ensure("no-such-claude").is_err());
        assert!(restart("no-such-claude").is_err());
        assert_eq!(refresh_installed("no-such-claude").ok(), Some(false));
    }

    #[test]
    fn the_real_unit_is_untouched_by_a_refresh_attempt() {
        let Some(unit) = dirs::config_dir().map(|d| d.join("systemd").join("user").join(UNIT_NAME))
        else {
            return;
        };
        let before = std::fs::metadata(&unit).and_then(|m| m.modified()).ok();
        assert_eq!(refresh_installed("/tmp/no-such-claude").ok(), Some(false));
        let after = std::fs::metadata(&unit).and_then(|m| m.modified()).ok();
        assert_eq!(before, after, "{} must not be rewritten by tests", unit.display());
    }
}
