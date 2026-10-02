//! Codex's primitives for the neutral [`crate::adapters::version_gate`]:
//! cycle a shared `codex app-server daemon` left behind by a CLI update.
//!
//! Busy is any live cctui codex session with an in-flight turn; a session
//! whose snapshot does not answer counts as busy. A turn seen in flight resets
//! the escalation clock. Per-session stdio app-servers are not touched by the
//! restart: they finish on the old binary.

use std::time::{Duration, Instant};

use serde_json::Value;

use crate::adapters::version_gate::{Busy, Decision, VersionGate};

const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DaemonVersions {
    pub cli: Option<String>,
    pub app_server: Option<String>,
}

/// Parse `codex app-server daemon version`, whose JSON carries `cliVersion`
/// and `appServerVersion` (absent when the daemon is not running).
#[must_use]
pub fn parse_daemon_version(stdout: &str) -> DaemonVersions {
    let field = |v: &Value, k: &str| {
        v.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned)
    };
    for line in stdout.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else { continue };
        if v.is_object() {
            return DaemonVersions {
                cli: field(&v, "cliVersion"),
                app_server: field(&v, "appServerVersion"),
            };
        }
    }
    DaemonVersions::default()
}

/// The app-server is the running version, the CLI the installed one.
#[must_use]
pub fn check(
    gate: &mut VersionGate,
    versions: &DaemonVersions,
    busy: Busy,
    now: Instant,
) -> Decision {
    if busy == Some(true) {
        gate.note_busy(now);
    }
    gate.check(versions.app_server.as_deref(), versions.cli.as_deref(), busy, false, now)
}

async fn run(bin: &str, args: &[&str]) -> anyhow::Result<std::process::Output> {
    let mut cmd = tokio::process::Command::new(bin);
    cmd.args(args)
        .env("PATH", crate::childenv::child_path())
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    crate::childenv::ScrubChildEnv::scrub_child_env(&mut cmd);
    Ok(tokio::time::timeout(PROBE_TIMEOUT, cmd.output()).await??)
}

pub async fn probe_versions(bin: &str) -> DaemonVersions {
    match run(bin, &["app-server", "daemon", "version"]).await {
        Ok(out) => parse_daemon_version(&String::from_utf8_lossy(&out.stdout)),
        Err(_) => DaemonVersions::default(),
    }
}

/// `codex app-server daemon restart`. The shared connection in
/// [`super::daemon`] reconnects to the same socket on its own.
pub async fn restart(bin: &str) -> anyhow::Result<()> {
    let out = run(bin, &["app-server", "daemon", "restart"]).await?;
    anyhow::ensure!(
        out.status.success(),
        "`codex app-server daemon restart` failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::version_gate::{ESCALATE_AFTER, parse_cli_version};

    const RUNNING: &str = r#"{"status":"running","backend":"pid","managedCodexPath":"/home/you/.codex/packages/standalone/current/codex","managedCodexVersion":"0.153.4","socketPath":"/home/you/.codex/app-server-control/app-server-control.sock","cliVersion":"0.155.0","appServerVersion":"0.153.4"}"#;

    fn cycle(r: &str, l: &str, escalated: bool) -> Decision {
        Decision::Cycle { running: r.into(), local: l.into(), escalated }
    }

    fn deferred(r: &str, l: &str) -> Decision {
        Decision::Deferred { running: r.into(), local: l.into() }
    }

    #[test]
    fn parses_daemon_version_json() {
        let v = parse_daemon_version(RUNNING);
        assert_eq!(v.cli.as_deref(), Some("0.155.0"));
        assert_eq!(v.app_server.as_deref(), Some("0.153.4"));
    }

    #[test]
    fn a_stopped_daemon_has_no_app_server_version() {
        let v = parse_daemon_version(
            "warning: stale pid\n{\"status\":\"stopped\",\"cliVersion\":\"0.155.0\"}\n",
        );
        assert_eq!(v.cli.as_deref(), Some("0.155.0"));
        assert_eq!(v.app_server, None);
        assert_eq!(parse_daemon_version("not json"), DaemonVersions::default());
    }

    #[test]
    fn parses_cli_banner() {
        assert_eq!(parse_cli_version("codex-cli 0.153.4\n").as_deref(), Some("0.153.4"));
        assert_eq!(parse_cli_version("codex-cli\n"), None);
    }

    #[test]
    fn check_reads_app_server_as_running_and_cli_as_local() {
        let mut g = VersionGate::default();
        let v = parse_daemon_version(RUNNING);
        let now = Instant::now();
        assert_eq!(check(&mut g, &v, Some(false), now), cycle("0.153.4", "0.155.0", false));
        assert_eq!(check(&mut g, &v, Some(true), now), deferred("0.153.4", "0.155.0"));
    }

    #[test]
    fn unknown_busy_escalates_after_the_window() {
        let mut g = VersionGate::default();
        let v = parse_daemon_version(RUNNING);
        let t0 = Instant::now();
        assert_eq!(check(&mut g, &v, None, t0), deferred("0.153.4", "0.155.0"));
        assert_eq!(
            check(&mut g, &v, None, t0 + ESCALATE_AFTER),
            cycle("0.153.4", "0.155.0", true)
        );
    }

    #[test]
    fn a_turn_in_flight_never_escalates_and_resets_the_clock() {
        let mut g = VersionGate::default();
        let v = parse_daemon_version(RUNNING);
        let t0 = Instant::now();
        let d = || deferred("0.153.4", "0.155.0");
        assert_eq!(check(&mut g, &v, None, t0), d());
        assert_eq!(check(&mut g, &v, Some(true), t0 + ESCALATE_AFTER), d());
        assert_eq!(check(&mut g, &v, None, t0 + ESCALATE_AFTER + Duration::from_secs(1)), d());
        assert_eq!(
            check(&mut g, &v, None, t0 + ESCALATE_AFTER * 2),
            cycle("0.153.4", "0.155.0", true)
        );
    }
}
