//! `cctui-daemon doctor`: the machine-health probe and the harness version
//! split, run in the foreground.
//!
//! `--grant` relaunches the launchd agents so macOS re-asks for the privacy
//! grants their last binary change dropped.

use std::path::Path;
use std::time::Duration;

use cctui_proto::harness::{HarnessVersion, MachineHealth, version_skew};

use crate::adapters::claude_code::version_gate::{DaemonStatus, parse_daemon_status};
use crate::adapters::version_gate::parse_cli_version;
use crate::config::Config;
use crate::machine_health;

const PROBE_TIMEOUT: Duration = Duration::from_secs(20);
const RELAUNCH_SETTLE: Duration = Duration::from_secs(5);

pub struct Opts {
    pub grant: bool,
    /// Relaunch cctui-daemon too, which ends live codex and opencode sessions.
    pub yes: bool,
}

struct ClaudeVersions {
    version: HarnessVersion,
    /// `None` when `claude daemon status` did not answer.
    status: Option<DaemonStatus>,
}

async fn run(bin: &str, args: &[&str]) -> Option<String> {
    let mut cmd = tokio::process::Command::new(bin);
    cmd.args(args)
        .env("PATH", crate::childenv::child_path())
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let out = tokio::time::timeout(PROBE_TIMEOUT, cmd.output()).await.ok()?.ok()?;
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

async fn claude_versions() -> ClaudeVersions {
    let cli = run("claude", &["--version"]).await.and_then(|o| parse_cli_version(&o));
    let status = run("claude", &["daemon", "status"]).await.map(|o| parse_daemon_status(&o));
    ClaudeVersions {
        version: HarnessVersion { cli, daemon: status.as_ref().and_then(|s| s.version.clone()) },
        status,
    }
}

fn print_health(verdict: &MachineHealth) {
    match verdict {
        MachineHealth::Ok => println!("machine health: ok"),
        MachineHealth::FolderDenied { detail } => {
            println!("machine health: folder access denied ({detail})");
        }
        MachineHealth::NetworkDenied { detail } => {
            println!("machine health: network denied ({detail})");
        }
    }
}

/// Unknown counts as busy: only a daemon that is not running, or one that
/// reports zero workers, may be relaunched.
fn relaunch_blocked(status: Option<&DaemonStatus>) -> bool {
    status.is_none_or(|s| s.version.is_some() && s.running_workers != Some(0))
}

async fn relaunch_claude_daemon(claude: &ClaudeVersions) {
    if relaunch_blocked(claude.status.as_ref()) {
        let workers = claude
            .status
            .as_ref()
            .and_then(|s| s.running_workers)
            .map_or_else(|| "an unknown number of".to_owned(), |n| n.to_string());
        println!(
            "claude daemon: {workers} live worker(s); not relaunched. Re-run `doctor --grant` \
             once they finish, or ask for a drain restart from the machine card"
        );
        return;
    }
    let res = tokio::task::spawn_blocking(|| {
        crate::adapters::claude_code::claude_service::restart("claude")
    })
    .await;
    match res {
        Ok(Ok(())) => println!("claude daemon: relaunched; accept its permission prompts"),
        Ok(Err(err)) => println!("claude daemon: relaunch failed: {err}"),
        Err(err) => println!("claude daemon: relaunch task failed: {err}"),
    }
}

fn relaunch_self(yes: bool) {
    if !yes {
        println!(
            "cctui-daemon: not relaunched (pass --yes; it ends live codex and opencode sessions \
             on this machine)"
        );
        return;
    }
    match crate::service::restart() {
        Ok(()) => println!("cctui-daemon: relaunched; accept its permission prompts"),
        Err(err) => println!("cctui-daemon: relaunch failed: {err}"),
    }
}

pub async fn run_doctor(config_path: &Path, opts: Opts) -> anyhow::Result<()> {
    let server_url = Config::load_from(&config_path.to_path_buf()).ok().map(|c| c.server_url);
    match &server_url {
        Some(url) => println!("server: {url}"),
        None => println!("server: not enrolled (network probe skipped)"),
    }

    let claude = claude_versions().await;
    println!(
        "claude: cli {} / daemon {}",
        claude.version.cli.as_deref().unwrap_or("not found"),
        claude.version.daemon.as_deref().unwrap_or("not running"),
    );
    let mut versions = cctui_proto::harness::HarnessVersions::default();
    versions.insert(cctui_proto::harness::HARNESS_CLAUDE_CODE, claude.version.clone());
    for (harness, cli, daemon) in version_skew(&versions) {
        println!("{harness}: daemon on {daemon}, CLI on {cli}: restart it when idle");
    }

    let folders = machine_health::probe_folders();
    for f in &folders {
        println!("probing folder {}", f.display());
    }
    let verdict = machine_health::probe(folders, server_url.as_deref()).await;
    print_health(&verdict);

    if !opts.grant {
        return Ok(());
    }
    if !cfg!(target_os = "macos") {
        println!("--grant is macOS-only: there is no privacy grant to re-request here");
        return Ok(());
    }
    relaunch_claude_daemon(&claude).await;
    relaunch_self(opts.yes);
    tokio::time::sleep(RELAUNCH_SETTLE).await;
    let after = machine_health::probe(machine_health::probe_folders(), server_url.as_deref()).await;
    print_health(&after);
    if !after.is_ok() {
        println!(
            "still denied: open System Settings → Privacy & Security and allow cctui-daemon and \
             claude under Local Network and the listed folders"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(version: Option<&str>, running_workers: Option<usize>) -> DaemonStatus {
        DaemonStatus { version: version.map(str::to_owned), running_workers }
    }

    #[test]
    fn live_or_unknown_workers_block_the_relaunch() {
        assert!(relaunch_blocked(Some(&status(Some("2.1.0"), Some(3)))));
        assert!(!relaunch_blocked(Some(&status(Some("2.1.0"), Some(0)))));
        assert!(
            relaunch_blocked(Some(&status(Some("2.1.0"), None))),
            "control unreachable: the count says nothing about what is alive"
        );
        assert!(relaunch_blocked(None), "a status that never answered is not idle");
        assert!(!relaunch_blocked(Some(&status(None, None))), "not running: nothing to protect");
    }
}
