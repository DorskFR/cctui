//! Claude's primitives for the neutral [`crate::adapters::version_gate`]:
//! cycle a `claude daemon` left behind by a CLI auto-update, but only while
//! nothing is running, since the sole remedy, `daemon stop --any`, kills every
//! background worker.
//!
//! Idle must be agreed by our filtered roster and the daemon's own
//! `bg workers: N running` count; unknown counts as busy. Only the roster
//! resets the escalation clock: a stale daemon's parked workers keep the
//! counts non-zero. A live job cctui did not start vetoes the escalation.
//!
//! Versions cannot come from the control socket: `cliVersion` rides each job,
//! so an idle daemon reports none.

use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

pub(super) use crate::adapters::version_gate::Decision;
use crate::adapters::version_gate::{self as gate, parse_cli_version};

const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CycleMethod {
    ManagedService,
    /// For a daemon started outside our unit (`origin: foreground`), which a
    /// unit restart would not touch.
    StopAny,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DaemonStatus {
    pub version: Option<String>,
    /// `None` when the count was not reported — treated as busy.
    pub running_workers: Option<usize>,
}

/// Parse the header of `claude daemon status`. Tolerant by construction: an
/// unrecognised line leaves that fact `None`, which steers away from cycling.
pub fn parse_daemon_status(stdout: &str) -> DaemonStatus {
    let mut out = DaemonStatus::default();
    for line in stdout.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("version:") {
            let v = rest.trim();
            if v.starts_with(|c: char| c.is_ascii_digit()) {
                out.version = Some(v.to_string());
            }
        } else if let Some(rest) = line.strip_prefix("bg workers:") {
            out.running_workers = parse_running_workers(rest);
        }
    }
    out
}

/// `2 running (control.sock), 2 in roster.json` -> `Some(2)`, but
/// `0 in roster.json (control unreachable)` -> `None`: with no socket the
/// count says nothing about what is alive.
fn parse_running_workers(rest: &str) -> Option<usize> {
    let mut toks = rest.split_whitespace().peekable();
    while let Some(tok) = toks.next() {
        if toks.peek() == Some(&"running") {
            return tok.parse().ok();
        }
    }
    None
}

/// Fold our roster size together with the daemon's own count. Either source
/// seeing work, or the daemon's count being unknown, means busy.
pub(super) fn live_workers(roster_len: usize, reported: Option<usize>) -> Option<usize> {
    reported.map(|n| n.max(roster_len))
}

pub(super) struct VersionGate {
    claude_bin: String,
    gate: Mutex<gate::VersionGate>,
}

impl VersionGate {
    pub(super) fn new(claude_bin: String) -> Self {
        Self { claude_bin, gate: Mutex::new(gate::VersionGate::default()) }
    }

    fn gate(&self) -> std::sync::MutexGuard<'_, gate::VersionGate> {
        self.gate.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Called from every roster poll that sees a busy session.
    pub(super) fn note_roster_busy(&self) {
        self.gate().note_busy(Instant::now());
    }

    async fn probe(&self, args: &[&str]) -> Option<String> {
        let out = tokio::time::timeout(
            PROBE_TIMEOUT,
            tokio::process::Command::new(&self.claude_bin)
                .args(args)
                .env("PATH", crate::childenv::child_path())
                .stdin(std::process::Stdio::null())
                .output(),
        )
        .await
        .ok()?
        .ok()?;
        Some(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Run one check if the interval has elapsed. `None` when the check was
    /// skipped or nothing needs doing.
    pub(super) async fn check(&self, roster_len: usize, native_live: bool) -> Option<Decision> {
        if !self.gate().due(Instant::now()) {
            return None;
        }
        let status = parse_daemon_status(&self.probe(&["daemon", "status"]).await?);
        let local = parse_cli_version(&self.probe(&["--version"]).await?);
        let busy = live_workers(roster_len, status.running_workers).map(|n| n > 0);
        let (decision, first_warning) = {
            let mut gate = self.gate();
            let decision = gate.check(
                status.version.as_deref(),
                local.as_deref(),
                busy,
                native_live,
                Instant::now(),
            );
            let first_warning = match &decision {
                Decision::Deferred { running, local } => gate.first_warning_for(running, local),
                _ => false,
            };
            (decision, first_warning)
        };
        match &decision {
            Decision::Nothing => None,
            Decision::Deferred { running, local } => {
                if first_warning {
                    tracing::warn!(
                        %running,
                        %local,
                        roster_len,
                        reported_workers = ?status.running_workers,
                        "claude daemon is older than the installed CLI; deferring the cycle \
                         until no workers are running"
                    );
                }
                Some(decision)
            }
            Decision::Cycle { running, local, escalated } => {
                if *escalated {
                    tracing::warn!(
                        %running,
                        %local,
                        roster_len,
                        reported_workers = ?status.running_workers,
                        "version mismatch deferred past the escalation window with a quiescent \
                         roster; cycling over non-zero worker counts"
                    );
                }
                Some(decision)
            }
        }
    }

    /// Bounce the supervisor. Prefers the managed unit; falls back to the
    /// CLI's own `stop --any` when the live daemon was not started by it.
    /// Best-effort: the caller re-establishes the socket either way.
    pub(super) async fn cycle(&self, method: CycleMethod) -> anyhow::Result<()> {
        match method {
            CycleMethod::ManagedService => {
                let bin = self.claude_bin.clone();
                tokio::task::spawn_blocking(move || super::claude_service::restart(&bin)).await?
            }
            CycleMethod::StopAny => {
                let out = tokio::time::timeout(
                    PROBE_TIMEOUT,
                    tokio::process::Command::new(&self.claude_bin)
                        .args(["daemon", "stop", "--any"])
                        .env("PATH", crate::childenv::child_path())
                        .stdin(std::process::Stdio::null())
                        .output(),
                )
                .await??;
                anyhow::ensure!(
                    out.status.success(),
                    "`claude daemon stop --any` failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                );
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUSY_STATUS: &str = "\
pid:     100001
version: 2.1.218
uptime:  161528s
origin:  foreground
config:  /home/you/.claude/daemon.json
log:     /home/you/.claude/daemon.log

bg sessions:
  sock dir:     /tmp/cc-daemon-9999/9a6631b1
  control.sock: reachable
  bg workers:   2 running (control.sock), 2 in roster.json
  roster.json:  updated 187s ago
";

    const IDLE_STATUS: &str = "\
pid:     100002
version: 2.1.220
uptime:  11s
origin:  foreground

bg sessions:
  control.sock: reachable
  bg workers:   0 running (control.sock), 0 in roster.json
";

    const DOWN_STATUS: &str = "\
not running

bg sessions:
  sock dir:     /tmp/cc-daemon-9999/f47e2fbc
  control.sock: unreachable (connect ENOENT)
  bg workers:   0 in roster.json (control unreachable)
";

    #[test]
    fn parses_cli_version_from_the_version_banner() {
        assert_eq!(parse_cli_version("2.1.218 (Claude Code)\n").as_deref(), Some("2.1.218"));
        assert_eq!(parse_cli_version(""), None);
        assert_eq!(parse_cli_version("some unexpected banner"), None);
    }

    #[test]
    fn parses_version_and_running_workers_from_status() {
        let busy = parse_daemon_status(BUSY_STATUS);
        assert_eq!(busy.version.as_deref(), Some("2.1.218"));
        assert_eq!(busy.running_workers, Some(2));

        let idle = parse_daemon_status(IDLE_STATUS);
        assert_eq!(idle.version.as_deref(), Some("2.1.220"));
        assert_eq!(idle.running_workers, Some(0));
    }

    #[test]
    fn a_stopped_daemon_yields_no_version_and_no_worker_count() {
        let down = parse_daemon_status(DOWN_STATUS);
        assert_eq!(down.version, None);
        // "0 in roster.json (control unreachable)" is NOT a running count: the
        // daemon could not see the socket, so it must not read as idle.
        assert_eq!(down.running_workers, None);
    }

    #[test]
    fn our_roster_can_veto_the_daemons_idle_report() {
        // The daemon says nothing is running but we are tracking a session:
        // busy wins, because either source seeing work means work exists.
        assert_eq!(live_workers(1, Some(0)), Some(1));
        assert_eq!(live_workers(0, Some(2)), Some(2));
        assert_eq!(live_workers(0, Some(0)), Some(0));
    }

    #[test]
    fn an_unreported_count_stays_unknown_even_with_an_empty_roster() {
        assert_eq!(live_workers(0, None), None);
    }

    /// Guards the fixtures above against CLI output drift. Needs a real
    /// `claude` with a running daemon, so it is not part of the normal run:
    /// `cargo test -p cctui-daemon -- --ignored parsers_match_the_live_cli`.
    #[test]
    #[ignore = "requires a live `claude` daemon"]
    fn parsers_match_the_live_cli() {
        let run = |args: &[&str]| {
            let out = std::process::Command::new("claude").args(args).output().unwrap();
            String::from_utf8_lossy(&out.stdout).into_owned()
        };
        let status = parse_daemon_status(&run(&["daemon", "status"]));
        assert!(status.version.is_some(), "no version parsed from live `claude daemon status`");
        assert!(status.running_workers.is_some(), "no worker count parsed; daemon down?");
        assert!(parse_cli_version(&run(&["--version"])).is_some());
    }

    fn busy(roster_len: usize, reported: Option<usize>) -> gate::Busy {
        live_workers(roster_len, reported).map(|n| n > 0)
    }

    fn deferred(r: &str, l: &str) -> Decision {
        Decision::Deferred { running: r.into(), local: l.into() }
    }

    #[test]
    fn a_mismatch_cycles_only_when_roster_and_daemon_agree_on_idle() {
        let g = VersionGate::new("claude".into());
        let t0 = Instant::now();
        let check = |b| g.gate().check(Some("2.1.212"), Some("2.1.220"), b, false, t0);
        assert_eq!(
            check(busy(0, Some(0))),
            Decision::Cycle {
                running: "2.1.212".into(),
                local: "2.1.220".into(),
                escalated: false
            }
        );
        assert_eq!(check(busy(1, Some(0))), deferred("2.1.212", "2.1.220"));
        assert_eq!(check(busy(0, None)), deferred("2.1.212", "2.1.220"));
        assert_eq!(
            g.gate().check(Some("2.1.220"), Some("2.1.220"), busy(0, Some(0)), false, t0),
            Decision::Nothing
        );
    }

    #[test]
    fn parked_workers_alone_do_not_hold_off_the_escalation() {
        let g = VersionGate::new("claude".into());
        let t0 = Instant::now();
        let check =
            |at| g.gate().check(Some("2.1.212"), Some("2.1.220"), busy(0, Some(2)), false, at);
        assert_eq!(check(t0), deferred("2.1.212", "2.1.220"));
        assert_eq!(
            check(t0 + gate::ESCALATE_AFTER),
            Decision::Cycle { running: "2.1.212".into(), local: "2.1.220".into(), escalated: true }
        );
    }

    #[test]
    fn roster_activity_resets_the_escalation_clock() {
        let g = VersionGate::new("claude".into());
        let t0 = Instant::now();
        let check =
            |at| g.gate().check(Some("2.1.212"), Some("2.1.220"), busy(0, Some(2)), false, at);
        check(t0);
        g.note_roster_busy();
        assert_eq!(check(t0 + gate::ESCALATE_AFTER), deferred("2.1.212", "2.1.220"));
    }

    #[test]
    fn a_live_foreign_job_vetoes_the_escalation() {
        let g = VersionGate::new("claude".into());
        let t0 = Instant::now();
        let check = |at, native| g.gate().check(Some("2.1.212"), Some("2.1.220"), None, native, at);
        check(t0, true);
        assert_eq!(
            check(t0 + gate::ESCALATE_AFTER, true),
            deferred("2.1.212", "2.1.220"),
            "a job cctui did not start must never be cycled away"
        );
        assert_eq!(
            check(t0 + gate::ESCALATE_AFTER, false),
            Decision::Cycle { running: "2.1.212".into(), local: "2.1.220".into(), escalated: true }
        );
    }
}
