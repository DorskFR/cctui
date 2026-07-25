//! Install & run the on-demand `claude daemon` under the OS user service
//! manager, decoupling its lifetime from cctui-daemon (CCT-590).
//!
//! The service manager parents the claude daemon: it supervises, restarts and
//! always reaps it, so cctui-daemon never parents the process (nothing to
//! zombie, no lifecycle coupling) and only ever *connects* to `control.sock`.
//!
//! The supervisor is kept ALWAYS RESIDENT (launchd `KeepAlive` / systemd
//! `Restart=on-failure`) rather than idle-shutting-down: the socket is then
//! always present, which removes the kickstart race (deliberate CCT-590
//! behavior change).
//!
//! CRITICAL (Linux): this must be its OWN systemd **user** unit, NOT part of
//! `cctui-daemon.service` — that unit runs `KillMode=control-group`, so
//! sharing its cgroup would make a cctui-daemon restart SIGTERM the claude
//! supervisor too (the coupling CCT-590 removes). A separate unit isolates it.

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

/// Ensure the managed claude-daemon service is installed, loaded and started.
///
/// Idempotent and cheap on the hot path: if the service is already active it
/// short-circuits after a single status probe, so calling this from every
/// kickstart poll does not churn the running supervisor. Only when the service
/// is absent does it write the unit/plist and start it. Best-effort — the
/// caller logs failures and retries; a still-missing socket surfaces as the
/// usual poll/dispatch error.
#[cfg(target_os = "macos")]
pub(super) fn ensure(claude_bin: &str) -> Result<()> {
    macos::ensure(claude_bin)
}
#[cfg(target_os = "linux")]
pub(super) fn ensure(claude_bin: &str) -> Result<()> {
    linux::ensure(claude_bin)
}
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(super) fn ensure(claude_bin: &str) -> Result<()> {
    let _ = claude_bin;
    anyhow::bail!("claude daemon service: unsupported OS")
}

/// Whether the managed service is currently the thing running the daemon. A
/// daemon started some other way (`origin: foreground`) survives a unit
/// restart untouched, so the caller must pick a different remedy.
#[cfg(target_os = "macos")]
pub(super) fn service_active() -> bool {
    macos::is_loaded()
}
#[cfg(target_os = "linux")]
pub(super) fn service_active() -> bool {
    linux::is_active()
}
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(super) const fn service_active() -> bool {
    false
}

/// Restart the managed claude-daemon service. Callers must have established
/// that no worker is running: this tears the supervisor down.
#[cfg(target_os = "macos")]
pub(super) fn restart(claude_bin: &str) -> Result<()> {
    macos::restart(claude_bin)
}
#[cfg(target_os = "linux")]
pub(super) fn restart(claude_bin: &str) -> Result<()> {
    linux::restart(claude_bin)
}
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(super) fn restart(claude_bin: &str) -> Result<()> {
    let _ = claude_bin;
    anyhow::bail!("claude daemon service: unsupported OS")
}

/// Whether an OS user service manager is usable here. Worker containers have
/// no systemd (`/run/systemd/system` absent, no user bus for `systemctl
/// --user` — CCT-629): the kickstarter must then spawn `claude daemon run` as
/// a direct child instead of calling [`ensure`].
#[cfg(target_os = "linux")]
pub(super) fn manager_available() -> bool {
    if std::env::var_os("SYSTEMD_OFFLINE").is_some_and(|v| v == "1") {
        return false;
    }
    std::path::Path::new("/run/systemd/system").is_dir()
}
#[cfg(target_os = "macos")]
pub(super) fn manager_available() -> bool {
    true
}
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(super) fn manager_available() -> bool {
    false
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
mod linux {
    use super::{UNIT_NAME, render_unit};
    use anyhow::{Context, Result, bail};
    use std::path::PathBuf;
    use std::process::Command;

    fn unit_dir() -> Result<PathBuf> {
        let base = dirs::config_dir().context("no $XDG_CONFIG_HOME / $HOME")?;
        Ok(base.join("systemd").join("user"))
    }

    pub(super) fn is_active() -> bool {
        Command::new("systemctl")
            .args(["--user", "is-active", "--quiet", UNIT_NAME])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
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

    pub(super) fn ensure(claude_bin: &str) -> Result<()> {
        if is_active() {
            return Ok(());
        }
        let dir = unit_dir()?;
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let path = dir.join(UNIT_NAME);
        std::fs::write(&path, render_unit(claude_bin))
            .with_context(|| format!("write {}", path.display()))?;
        systemctl(&["daemon-reload"])?;
        // `enable --now` is idempotent: it enables the unit and starts it if
        // not already running. Its own user unit -> its own cgroup, never
        // cctui-daemon.service's KillMode=control-group cgroup.
        systemctl(&["enable", "--now", UNIT_NAME])?;
        tracing::info!(unit = UNIT_NAME, "installed and started managed claude daemon");
        Ok(())
    }

    pub(super) fn restart(claude_bin: &str) -> Result<()> {
        ensure(claude_bin)?;
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

    pub(super) fn is_loaded() -> bool {
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
        // Augmented PATH baked in (CCT-138 launchd minimal-PATH fix).
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
}
