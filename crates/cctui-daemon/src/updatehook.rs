//! Deploy a new cctui the way *this* deployment is normally deployed.
//!
//! The server has no idea whether it lives in a Kubernetes Deployment, a
//! Compose project or a systemd unit, and it should not have to guess. So the
//! knowledge stays where it belongs: on the machine, as one shell command the
//! operator wrote once (`CCTUI_UPDATE_COMMAND`). The server only ever says
//! "please update to v1.2.3"; this module does it.
//!
//! What that buys over handing the job to a YOLO agent: the same bytes run
//! every time, the operator can read them before they run, the daemon needs no
//! model and no account, and a failure has a defined answer instead of a
//! transcript.
//!
//! One run, in order:
//!
//!   1. `Running` — execute `CCTUI_UPDATE_COMMAND`, capped by
//!      `CCTUI_UPDATE_TIMEOUT_SECS`.
//!   2. `Verifying` — poll the health endpoint until it reports the target
//!      version, capped by `CCTUI_UPDATE_HEALTH_TIMEOUT_SECS`. An update that
//!      "succeeded" but did not move the served version did not succeed.
//!   3. `Succeeded`, or `RollingBack` → `RolledBack` when
//!      `CCTUI_UPDATE_ROLLBACK_COMMAND` is set, or `Failed` when it is not.
//!
//! Every phase is reported to the server over HTTP. That is not a detail: step
//! 1 restarts the server, so the process that asked for the run is usually
//! gone by step 2. Reports retry across that gap, and the run row in Postgres
//! is what actually remembers.

// `Ops` is generic, so clippy cannot prove any implementor's futures are
// `Send`; the one that matters (`LiveOps`) is, which is why `spawn` below
// compiles under `tokio::spawn`, and the test double deliberately is not.
#![allow(clippy::future_not_send)]

use std::collections::BTreeMap;
use std::time::Duration;

use cctui_proto::updatehook::{UpdateHookPhase, UpdateHookReport, tail};

/// Default cap on the update command itself. Pulling and rolling out an image
/// is minutes, not seconds; anything past this is stuck, not slow.
const DEFAULT_TIMEOUT: Duration = Duration::from_mins(15);

/// Default cap on waiting for the new version to answer.
const DEFAULT_HEALTH_TIMEOUT: Duration = Duration::from_mins(10);

/// Gap between two health probes while verifying.
const HEALTH_INTERVAL: Duration = Duration::from_secs(5);

/// Per-probe HTTP timeout. Short: during a rollout the endpoint is expected to
/// refuse connections, and we would rather probe again than hang on one.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a terminal report keeps retrying. It must outlive the restart it
/// is reporting on, otherwise the run stays "running" in the UI forever.
const REPORT_DEADLINE: Duration = Duration::from_mins(10);

/// Gap between two report attempts.
const REPORT_RETRY_INTERVAL: Duration = Duration::from_secs(5);

/// The machine's update contract, as configured on the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookConfig {
    /// Shell command that deploys the requested version.
    pub command: String,
    /// Shell command that puts the deployment back as it was. Optional: not
    /// every deployment can roll back, and a wrong rollback is worse than
    /// none.
    pub rollback: Option<String>,
    /// Working directory for both commands; the daemon's cwd when unset.
    pub dir: Option<String>,
    pub timeout: Duration,
    /// URL polled until it reports the target version.
    pub health_url: String,
    pub health_timeout: Duration,
}

fn env_secs(key: &str, default: Duration) -> Duration {
    match std::env::var(key).ok().and_then(|s| s.trim().parse::<u64>().ok()) {
        Some(secs) if secs > 0 => Duration::from_secs(secs),
        _ => default,
    }
}

fn env_trimmed(key: &str) -> Option<String> {
    std::env::var(key).ok().map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}

/// The default health endpoint: this server's daemon-scoped version probe.
///
/// `/api/v1/version` needs a *user* token, which a daemon does not have, so the
/// daemon-scoped sibling is the one it can actually call.
#[must_use]
pub fn default_health_url(server_url: &str) -> String {
    format!("{}/api/v1/daemon/version", server_url.trim_end_matches('/'))
}

impl HookConfig {
    /// Read the hook from the environment, or `None` when this machine has no
    /// update command configured (the overwhelmingly common case: only the
    /// machine that hosts cctui has one).
    #[must_use]
    pub fn from_env(server_url: &str) -> Option<Self> {
        let command = env_trimmed("CCTUI_UPDATE_COMMAND")?;
        Some(Self {
            command,
            rollback: env_trimmed("CCTUI_UPDATE_ROLLBACK_COMMAND"),
            dir: env_trimmed("CCTUI_UPDATE_DIR"),
            timeout: env_secs("CCTUI_UPDATE_TIMEOUT_SECS", DEFAULT_TIMEOUT),
            health_url: env_trimmed("CCTUI_UPDATE_HEALTH_URL")
                .unwrap_or_else(|| default_health_url(server_url)),
            health_timeout: env_secs("CCTUI_UPDATE_HEALTH_TIMEOUT_SECS", DEFAULT_HEALTH_TIMEOUT),
        })
    }
}

/// Whether this machine advertises a deterministic update hook.
///
/// Cheap enough to call on every heartbeat, and re-read each time so an
/// operator who sets the variable and restarts the daemon is picked up without
/// a server-side change.
#[must_use]
pub fn configured() -> bool {
    env_trimmed("CCTUI_UPDATE_COMMAND").is_some()
}

/// What a finished command left behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecOutcome {
    /// `None` when the command was killed on timeout.
    pub exit_code: Option<i32>,
    pub output: String,
    /// Set when the command could not be run or was killed, for the detail
    /// line; distinct from a command that ran and failed.
    pub error: Option<String>,
}

impl ExecOutcome {
    fn ok(&self) -> bool {
        self.error.is_none() && self.exit_code == Some(0)
    }

    fn describe(&self) -> String {
        match (&self.error, self.exit_code) {
            (Some(err), _) => err.clone(),
            (None, Some(0)) => "exited 0".to_owned(),
            (None, Some(code)) => format!("exited {code}"),
            (None, None) => "exited without a status".to_owned(),
        }
    }
}

/// The side effects a run needs, behind a trait.
///
/// The phase sequence below can then be tested without spawning shells or
/// opening sockets, and the sequencing is the part that has to be right: it
/// decides when a deployment gets rolled back.
#[allow(async_fn_in_trait)]
pub trait Ops {
    /// Run `command` to completion (or `timeout`), merging stdout and stderr.
    async fn exec(
        &self,
        command: &str,
        dir: Option<&str>,
        env: &BTreeMap<String, String>,
        timeout: Duration,
    ) -> ExecOutcome;

    /// One health probe: the version the deployment currently serves, or
    /// `None` when it did not answer (expected mid-restart).
    async fn probe(&self, url: &str) -> Option<String>;

    /// Publish one progress report. Implementations retry; a terminal report
    /// that never lands leaves the run looking stuck.
    async fn report(&self, report: UpdateHookReport);

    /// Sleep, so tests do not.
    async fn sleep(&self, d: Duration);

    /// Elapsed time since the run started, so tests can drive the deadline.
    fn elapsed(&self) -> Duration;
}

/// Environment handed to both commands, so a hook script can act on the
/// request instead of hardcoding a version.
fn hook_env(version: &str, release_url: &str, run_id: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("CCTUI_UPDATE_VERSION".to_owned(), version.to_owned()),
        ("CCTUI_UPDATE_RELEASE_URL".to_owned(), release_url.to_owned()),
        ("CCTUI_UPDATE_RUN_ID".to_owned(), run_id.to_owned()),
    ])
}

fn report(phase: UpdateHookPhase, detail: String, out: &ExecOutcome) -> UpdateHookReport {
    UpdateHookReport {
        phase,
        exit_code: out.exit_code,
        detail,
        output_tail: (!out.output.is_empty()).then(|| tail(&out.output)),
    }
}

/// Run one update hook end to end, reporting each phase. Returns the terminal
/// phase it reached.
pub async fn run<O: Ops>(
    ops: &O,
    cfg: &HookConfig,
    version: &str,
    release_url: &str,
    run_id: &str,
) -> UpdateHookPhase {
    let env = hook_env(version, release_url, run_id);

    ops.report(UpdateHookReport {
        phase: UpdateHookPhase::Running,
        exit_code: None,
        detail: format!("running the update command for v{version}"),
        output_tail: None,
    })
    .await;

    let out = ops.exec(&cfg.command, cfg.dir.as_deref(), &env, cfg.timeout).await;
    if !out.ok() {
        let detail = format!("update command {}", out.describe());
        return rollback(ops, cfg, &env, detail, out).await;
    }

    ops.report(report(
        UpdateHookPhase::Verifying,
        format!("update command exited 0; waiting for v{version} to answer"),
        &out,
    ))
    .await;

    match verify(ops, cfg, version).await {
        Ok(()) => {
            ops.report(report(
                UpdateHookPhase::Succeeded,
                format!("deployment is serving v{version}"),
                &out,
            ))
            .await;
            UpdateHookPhase::Succeeded
        }
        Err(detail) => rollback(ops, cfg, &env, detail, out).await,
    }
}

/// Poll until the deployment serves `version`, or the health budget runs out.
/// The error is the detail line explaining what was seen instead.
async fn verify<O: Ops>(ops: &O, cfg: &HookConfig, version: &str) -> Result<(), String> {
    let deadline = ops.elapsed() + cfg.health_timeout;
    let mut last: Option<String> = None;
    loop {
        if let Some(seen) = ops.probe(&cfg.health_url).await {
            if seen == version {
                return Ok(());
            }
            last = Some(seen);
        }
        if ops.elapsed() >= deadline {
            let secs = cfg.health_timeout.as_secs();
            return Err(last.map_or_else(
                || format!("health check never answered within {secs}s"),
                |seen| format!("health check still reports v{seen} after {secs}s, not v{version}"),
            ));
        }
        ops.sleep(HEALTH_INTERVAL).await;
    }
}

/// Put the deployment back, if the operator gave us a way to. `detail` is why
/// we are here; it stays in the final report either way, because "it rolled
/// back" is useless without "because the health check never answered".
async fn rollback<O: Ops>(
    ops: &O,
    cfg: &HookConfig,
    env: &BTreeMap<String, String>,
    detail: String,
    out: ExecOutcome,
) -> UpdateHookPhase {
    let Some(cmd) = cfg.rollback.as_deref() else {
        ops.report(report(
            UpdateHookPhase::Failed,
            format!("{detail}; no rollback command configured"),
            &out,
        ))
        .await;
        return UpdateHookPhase::Failed;
    };

    ops.report(report(UpdateHookPhase::RollingBack, format!("{detail}; rolling back"), &out)).await;

    let back = ops.exec(cmd, cfg.dir.as_deref(), env, cfg.timeout).await;
    if back.ok() {
        ops.report(report(
            UpdateHookPhase::RolledBack,
            format!("{detail}; rolled back to the previous version"),
            &back,
        ))
        .await;
        UpdateHookPhase::RolledBack
    } else {
        // Both commands failed: this deployment needs a human, and saying so
        // is more useful than reporting a rollback that did not happen.
        ops.report(report(
            UpdateHookPhase::Failed,
            format!(
                "{detail}; rollback command {} — this deployment needs a human",
                back.describe()
            ),
            &back,
        ))
        .await;
        UpdateHookPhase::Failed
    }
}

/// The real [`Ops`]: shells out, probes over HTTP, reports to the server.
pub struct LiveOps {
    http: reqwest::Client,
    report_url: String,
    machine_key: String,
    started: std::time::Instant,
}

impl LiveOps {
    #[must_use]
    pub fn new(http: reqwest::Client, server_url: &str, machine_key: &str, run_id: &str) -> Self {
        Self {
            http,
            report_url: format!(
                "{}/api/v1/daemon/update-hook/{run_id}",
                server_url.trim_end_matches('/')
            ),
            machine_key: machine_key.to_owned(),
            started: std::time::Instant::now(),
        }
    }
}

impl Ops for LiveOps {
    async fn exec(
        &self,
        command: &str,
        dir: Option<&str>,
        env: &BTreeMap<String, String>,
        timeout: Duration,
    ) -> ExecOutcome {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c").arg(command).kill_on_drop(true);
        if let Some(dir) = dir {
            cmd.current_dir(dir);
        }
        for (k, v) in env {
            cmd.env(k, v);
        }
        // Merged, because a hook's useful output is interleaved by nature: the
        // tool's progress on stdout and its complaint on stderr.
        cmd.stderr(std::process::Stdio::piped()).stdout(std::process::Stdio::piped());

        match tokio::time::timeout(timeout, cmd.output()).await {
            Ok(Ok(out)) => {
                let mut merged = String::from_utf8_lossy(&out.stdout).into_owned();
                if !out.stderr.is_empty() {
                    merged.push_str(&String::from_utf8_lossy(&out.stderr));
                }
                ExecOutcome { exit_code: out.status.code(), output: merged, error: None }
            }
            Ok(Err(err)) => ExecOutcome {
                exit_code: None,
                output: String::new(),
                error: Some(format!("could not run the command: {err}")),
            },
            Err(_) => ExecOutcome {
                exit_code: None,
                output: String::new(),
                error: Some(format!("timed out after {}s", timeout.as_secs())),
            },
        }
    }

    async fn probe(&self, url: &str) -> Option<String> {
        #[derive(serde::Deserialize)]
        struct Probe {
            version: String,
        }
        let resp = self
            .http
            .get(url)
            .bearer_auth(&self.machine_key)
            .timeout(PROBE_TIMEOUT)
            .send()
            .await
            .ok()?;
        if !resp.status().is_success() {
            return None;
        }
        resp.json::<Probe>().await.ok().map(|p| p.version)
    }

    async fn report(&self, report: UpdateHookReport) {
        let terminal = report.phase.is_terminal();
        let deadline = std::time::Instant::now() + REPORT_DEADLINE;
        loop {
            let sent = self
                .http
                .post(&self.report_url)
                .bearer_auth(&self.machine_key)
                .json(&report)
                .send()
                .await
                .is_ok_and(|r| r.status().is_success());
            if sent {
                return;
            }
            // A progress report is a nicety; a terminal one is the run's only
            // record, so it keeps trying across the restart it is reporting on.
            if !terminal || std::time::Instant::now() >= deadline {
                tracing::warn!(phase = ?report.phase, "update hook report could not be delivered");
                return;
            }
            tokio::time::sleep(REPORT_RETRY_INTERVAL).await;
        }
    }

    async fn sleep(&self, d: Duration) {
        tokio::time::sleep(d).await;
    }

    fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }
}

/// Handle a `RunUpdateHook` frame: run the hook off the frame loop, or report
/// straight away that this machine has none.
pub fn spawn(
    http: reqwest::Client,
    server_url: String,
    machine_key: String,
    run_id: uuid::Uuid,
    version: String,
    release_url: String,
) {
    tokio::spawn(async move {
        let ops = LiveOps::new(http, &server_url, &machine_key, &run_id.to_string());
        let Some(cfg) = HookConfig::from_env(&server_url) else {
            // The server thought this machine had a hook (a stale heartbeat
            // flag, or the variable was removed). Say so rather than leaving
            // the run hanging: the admin can fall back to the agent.
            ops.report(UpdateHookReport {
                phase: UpdateHookPhase::Failed,
                exit_code: None,
                detail: "no update hook configured on this machine (CCTUI_UPDATE_COMMAND is unset)"
                    .to_owned(),
                output_tail: None,
            })
            .await;
            return;
        };
        tracing::info!(%run_id, %version, "running the deployment's update hook");
        let phase = run(&ops, &cfg, &version, &release_url, &run_id.to_string()).await;
        tracing::info!(%run_id, ?phase, "update hook finished");
    });
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    /// Scripted [`Ops`]: canned exec outcomes and probe answers, recording the
    /// phases the run walked through.
    struct FakeOps {
        execs: RefCell<Vec<ExecOutcome>>,
        probes: RefCell<Vec<Option<String>>>,
        ran: RefCell<Vec<String>>,
        phases: RefCell<Vec<UpdateHookPhase>>,
        details: RefCell<Vec<String>>,
        elapsed: RefCell<Duration>,
    }

    impl FakeOps {
        fn new(execs: Vec<ExecOutcome>, probes: Vec<Option<String>>) -> Self {
            Self {
                execs: RefCell::new(execs),
                probes: RefCell::new(probes),
                ran: RefCell::new(Vec::new()),
                phases: RefCell::new(Vec::new()),
                details: RefCell::new(Vec::new()),
                elapsed: RefCell::new(Duration::ZERO),
            }
        }
    }

    fn ok_exec() -> ExecOutcome {
        ExecOutcome { exit_code: Some(0), output: "rolled out".into(), error: None }
    }

    fn failed_exec() -> ExecOutcome {
        ExecOutcome { exit_code: Some(1), output: "boom".into(), error: None }
    }

    impl Ops for FakeOps {
        async fn exec(
            &self,
            command: &str,
            _dir: Option<&str>,
            env: &BTreeMap<String, String>,
            _timeout: Duration,
        ) -> ExecOutcome {
            assert_eq!(env.get("CCTUI_UPDATE_VERSION").map(String::as_str), Some("1.2.3"));
            self.ran.borrow_mut().push(command.to_owned());
            self.execs.borrow_mut().remove(0)
        }

        async fn probe(&self, _url: &str) -> Option<String> {
            let mut probes = self.probes.borrow_mut();
            if probes.is_empty() { None } else { probes.remove(0) }
        }

        async fn report(&self, report: UpdateHookReport) {
            self.phases.borrow_mut().push(report.phase);
            self.details.borrow_mut().push(report.detail);
        }

        async fn sleep(&self, d: Duration) {
            *self.elapsed.borrow_mut() += d;
        }

        fn elapsed(&self) -> Duration {
            *self.elapsed.borrow()
        }
    }

    fn cfg(rollback: Option<&str>) -> HookConfig {
        HookConfig {
            command: "deploy.sh".into(),
            rollback: rollback.map(str::to_owned),
            dir: None,
            timeout: Duration::from_mins(1),
            health_url: "http://server/api/v1/daemon/version".into(),
            health_timeout: Duration::from_secs(30),
        }
    }

    #[tokio::test]
    async fn a_clean_update_verified_by_the_health_check_succeeds() {
        // The deployment answers with the old version twice (restart in
        // progress) before serving the new one.
        let ops =
            FakeOps::new(vec![ok_exec()], vec![None, Some("1.2.2".into()), Some("1.2.3".into())]);
        let phase = run(&ops, &cfg(Some("undo.sh")), "1.2.3", "https://x/y", "run-1").await;
        assert_eq!(phase, UpdateHookPhase::Succeeded);
        assert_eq!(
            *ops.phases.borrow(),
            vec![UpdateHookPhase::Running, UpdateHookPhase::Verifying, UpdateHookPhase::Succeeded]
        );
        // The rollback command was never run.
        assert_eq!(*ops.ran.borrow(), vec!["deploy.sh"]);
    }

    #[tokio::test]
    async fn a_failing_update_command_rolls_back_without_verifying() {
        let ops = FakeOps::new(vec![failed_exec(), ok_exec()], vec![]);
        let phase = run(&ops, &cfg(Some("undo.sh")), "1.2.3", "https://x/y", "run-1").await;
        assert_eq!(phase, UpdateHookPhase::RolledBack);
        assert_eq!(
            *ops.phases.borrow(),
            vec![
                UpdateHookPhase::Running,
                UpdateHookPhase::RollingBack,
                UpdateHookPhase::RolledBack
            ]
        );
        assert_eq!(*ops.ran.borrow(), vec!["deploy.sh", "undo.sh"]);
        assert!(ops.details.borrow().last().unwrap().contains("exited 1"));
    }

    #[tokio::test]
    async fn a_command_that_exits_zero_without_moving_the_version_rolls_back() {
        // The whole point of the health check: `kubectl apply` happily exits 0
        // against an object that never rolled out.
        let ops = FakeOps::new(vec![ok_exec(), ok_exec()], vec![Some("1.2.2".into())]);
        let phase = run(&ops, &cfg(Some("undo.sh")), "1.2.3", "https://x/y", "run-1").await;
        assert_eq!(phase, UpdateHookPhase::RolledBack);
        assert_eq!(*ops.ran.borrow(), vec!["deploy.sh", "undo.sh"]);
        assert!(ops.details.borrow().last().unwrap().contains("still reports v1.2.2"));
    }

    #[tokio::test]
    async fn a_deployment_that_never_answers_fails_when_no_rollback_exists() {
        let ops = FakeOps::new(vec![ok_exec()], vec![]);
        let phase = run(&ops, &cfg(None), "1.2.3", "https://x/y", "run-1").await;
        assert_eq!(phase, UpdateHookPhase::Failed);
        assert!(ops.details.borrow().last().unwrap().contains("never answered"));
        assert!(ops.details.borrow().last().unwrap().contains("no rollback command"));
    }

    #[tokio::test]
    async fn a_failed_rollback_says_a_human_is_needed() {
        let ops = FakeOps::new(vec![failed_exec(), failed_exec()], vec![]);
        let phase = run(&ops, &cfg(Some("undo.sh")), "1.2.3", "https://x/y", "run-1").await;
        assert_eq!(phase, UpdateHookPhase::Failed);
        assert!(ops.details.borrow().last().unwrap().contains("needs a human"));
    }

    #[test]
    fn health_url_defaults_to_the_daemon_scoped_probe() {
        assert_eq!(
            default_health_url("https://cctui.example.com/"),
            "https://cctui.example.com/api/v1/daemon/version"
        );
    }

    #[test]
    fn a_timed_out_command_reads_as_an_error_not_as_exit_zero() {
        let timed_out = ExecOutcome {
            exit_code: None,
            output: String::new(),
            error: Some("timed out after 900s".into()),
        };
        assert!(!timed_out.ok());
        assert_eq!(timed_out.describe(), "timed out after 900s");
        assert!(!ExecOutcome { exit_code: None, output: String::new(), error: None }.ok());
    }
}
