//! The agent process: spawned as its own group so a kill reaches whatever
//! the launcher forked, stderr drained into the traffic ring.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use crate::adapters::traffic_rings::TrafficRings;
use crate::childenv::ScrubChildEnv;

const TERM_GRACE: Duration = Duration::from_secs(3);
const GROUP_REAP_POLLS: u32 = 50;
const GROUP_REAP_POLL_EVERY: Duration = Duration::from_millis(20);

/// Everything needed to exec one agent.
#[derive(Debug, Clone)]
pub struct Launch {
    pub bin: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
}

pub struct Spawned {
    pub child: Child,
    pub stdin: ChildStdin,
    pub stdout: ChildStdout,
}

pub fn spawn(launch: &Launch, rings: &Arc<TrafficRings>) -> anyhow::Result<Spawned> {
    anyhow::ensure!(launch.cwd.is_dir(), "working_dir does not exist: {}", launch.cwd.display());
    let mut cmd = Command::new(&launch.bin);
    cmd.args(&launch.args)
        .current_dir(&launch.cwd)
        .envs(&launch.env)
        .env("PATH", crate::childenv::child_path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd.scrub_child_env();
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child =
        cmd.spawn().with_context(|| format!("spawn `{} {}`", launch.bin, launch.args.join(" ")))?;
    let stdin = child.stdin.take().context("agent stdin")?;
    let stdout = child.stdout.take().context("agent stdout")?;
    if let Some(stderr) = child.stderr.take() {
        let rings = Arc::clone(rings);
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                rings.note_stderr(&line);
                tracing::debug!(target: "acp_agent_stderr", "{line}");
            }
        });
    }
    Ok(Spawned { child, stdin, stdout })
}

/// Terminate the agent's whole process group: SIGTERM, a grace period, then
/// SIGKILL until the group is gone. Returns once the direct child is reaped.
pub async fn shutdown(child: &mut Child) {
    let pgid = child.id().and_then(|p| i32::try_from(p).ok());
    signal_group(pgid, rustix::process::Signal::TERM);
    if tokio::time::timeout(TERM_GRACE, child.wait()).await.is_err() {
        signal_group(pgid, rustix::process::Signal::KILL);
        let _ = child.start_kill();
        let _ = child.wait().await;
    }
    let Some(pgid) = pgid else { return };
    for _ in 0..GROUP_REAP_POLLS {
        if !group_alive(pgid) {
            return;
        }
        signal_group(Some(pgid), rustix::process::Signal::KILL);
        tokio::time::sleep(GROUP_REAP_POLL_EVERY).await;
    }
    if group_alive(pgid) {
        tracing::error!(pgid, "acp agent process group survived SIGKILL");
    }
}

/// A fully reaped group yields `ESRCH`.
#[must_use]
pub fn group_alive(pgid: i32) -> bool {
    rustix::process::Pid::from_raw(pgid)
        .is_some_and(|p| rustix::process::test_kill_process_group(p).is_ok())
}

fn signal_group(pgid: Option<i32>, signal: rustix::process::Signal) {
    if let Some(pid) = pgid.and_then(rustix::process::Pid::from_raw) {
        let _ = rustix::process::kill_process_group(pid, signal);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_missing_working_dir_is_refused_before_any_exec() {
        let launch = Launch {
            bin: "/definitely/not/here".to_owned(),
            args: vec![],
            cwd: PathBuf::from("/no/such/dir/for/acp"),
            env: BTreeMap::new(),
        };
        let err = spawn(&launch, &Arc::new(TrafficRings::default())).unwrap_err();
        assert!(err.to_string().contains("working_dir does not exist"), "{err}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shutdown_takes_the_whole_group_down() {
        let tmp = tempfile::tempdir().unwrap();
        let launch = Launch {
            bin: "/bin/sh".to_owned(),
            args: vec!["-c".to_owned(), "sleep 300 & sleep 300".to_owned()],
            cwd: tmp.path().to_path_buf(),
            env: BTreeMap::new(),
        };
        let rings = Arc::new(TrafficRings::default());
        let mut spawned = spawn(&launch, &rings).unwrap();
        let pgid = i32::try_from(spawned.child.id().unwrap()).unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(group_alive(pgid));
        shutdown(&mut spawned.child).await;
        assert!(!group_alive(pgid), "the backgrounded sleep must die with its leader");
    }
}
