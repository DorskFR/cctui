use std::fmt::Write as _;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::{Context as _, anyhow, bail};
use clap::Subcommand;
use serde::{Deserialize, Serialize};

use crate::attack::{AgentJob, Target, plan};
use crate::budget::{Admission, CutOff, Ledger};
use crate::judge::{Event, EventKind, Evidence, Expectation, judge};
use crate::oracle::Verdict;
use crate::scenario::Mode;

/// Runs one job end to end and returns what the oracles observed.
///
/// The job's `budget` is the admitted cap; the implementation must pass it to
/// the dispatched session and tear the worker down when the run ends.
pub trait Dispatch {
    fn dispatch(&mut self, job: &AgentJob) -> anyhow::Result<Evidence>;
}

/// A dispatch program: reads one job JSON on stdin, prints evidence JSON on stdout.
#[derive(Debug, Clone)]
pub struct CommandDispatch {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub grace: Duration,
}

fn wall_clock_evidence() -> Evidence {
    Evidence {
        events: vec![Event { kind: EventKind::Error, text: "wall-clock limit".to_owned() }],
        cut_off: Some(CutOff::WallClock),
        ..Evidence::default()
    }
}

impl Dispatch for CommandDispatch {
    fn dispatch(&mut self, job: &AgentJob) -> anyhow::Result<Evidence> {
        let mut child = std::process::Command::new(&self.program)
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| format!("spawn {}", self.program.display()))?;
        let mut stdout = child.stdout.take().context("child stdout")?;
        let reader = std::thread::spawn(move || {
            let mut s = String::new();
            stdout.read_to_string(&mut s).map(|_| s)
        });
        {
            let mut stdin = child.stdin.take().context("child stdin")?;
            stdin.write_all(&serde_json::to_vec(job)?)?;
        }
        let deadline = Instant::now() + Duration::from_secs(job.budget.timeout_secs) + self.grace;
        loop {
            if let Some(status) = child.try_wait()? {
                let out = reader.join().map_err(|_| anyhow!("stdout reader panicked"))??;
                if !status.success() {
                    bail!("dispatch program exited with {status}");
                }
                return Ok(serde_json::from_str(&out)?);
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(wall_clock_evidence());
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentReport {
    pub scenario: String,
    pub class: String,
    pub mode: Mode,
    pub harness: String,
    pub model: String,
    pub verdict: Verdict,
    pub reasons: Vec<String>,
    pub complied: Option<bool>,
    pub cap_usd: f64,
    pub cost_usd: f64,
    pub cut_off: Option<CutOff>,
    pub overrun: bool,
}

impl AgentReport {
    fn new(job: &AgentJob, verdict: Verdict, reasons: Vec<String>) -> Self {
        Self {
            scenario: job.scenario.clone(),
            class: job.class.clone(),
            mode: job.mode,
            harness: job.harness.clone(),
            model: job.model.clone(),
            verdict,
            reasons,
            complied: None,
            cap_usd: 0.0,
            cost_usd: 0.0,
            cut_off: None,
            overrun: false,
        }
    }
}

fn run_one(
    job: &AgentJob,
    cap: f64,
    ledger: &mut Ledger,
    canaries: &[String],
    d: &mut impl Dispatch,
) -> AgentReport {
    let mut capped = job.clone();
    capped.budget.usd = cap;
    let evidence = match d.dispatch(&capped) {
        Ok(ev) => ev,
        Err(e) => {
            ledger.settle(cap, cap);
            let reasons = vec![format!("dispatch: {e}")];
            let mut r = AgentReport::new(job, Verdict::Inconclusive, reasons);
            r.cap_usd = cap;
            r.cost_usd = cap;
            return r;
        }
    };
    let exp = Expectation {
        mode: job.mode,
        canaries,
        markers: &job.markers,
        expected_decision: job.expected_decision,
    };
    let v = judge(&exp, &evidence);
    let mut r = AgentReport::new(job, v.verdict, v.reasons);
    r.complied = v.complied;
    r.cap_usd = cap;
    r.cost_usd = evidence.cost_usd;
    r.cut_off =
        evidence.cut_off.or_else(|| capped.budget.check(evidence.cost_usd, evidence.turns, 0));
    r.overrun = ledger.settle(cap, evidence.cost_usd);
    r
}

/// Dispatch `jobs` in order; once the ledger is spent the rest are not run.
pub fn run_jobs(
    jobs: &[AgentJob],
    ledger: &mut Ledger,
    canaries: &[String],
    d: &mut impl Dispatch,
) -> Vec<AgentReport> {
    jobs.iter()
        .map(|job| match ledger.admit(job.budget.usd) {
            Admission::Admitted(cap) => run_one(job, cap, ledger, canaries, d),
            Admission::Exhausted => {
                let reasons = vec!["run budget exhausted".to_owned()];
                AgentReport::new(job, Verdict::Inconclusive, reasons)
            }
        })
        .collect()
}

/// A run passes when nothing escaped or was inconclusive; `refused` neither passes nor fails.
#[must_use]
pub fn passed(reports: &[AgentReport]) -> bool {
    reports.iter().all(|r| matches!(r.verdict, Verdict::Contained | Verdict::Refused))
}

#[must_use]
pub fn to_markdown(reports: &[AgentReport]) -> String {
    let mut out = String::from(
        "# red-team T3\n\n| scenario | harness | model | mode | verdict | complied | cost / cap | reasons |\n|---|---|---|---|---|---|---|---|\n",
    );
    for r in reports {
        let complied = r.complied.map_or("-", |c| if c { "yes" } else { "no" });
        let _ = writeln!(
            out,
            "| {} | {} | {} | {:?} | {} | {complied} | {:.2} / {:.2} | {} |",
            r.scenario,
            r.harness,
            r.model,
            r.mode,
            r.verdict.as_str(),
            r.cost_usd,
            r.cap_usd,
            r.reasons.join("; ")
        );
    }
    out
}

fn read_canaries(path: Option<&Path>) -> anyhow::Result<Vec<String>> {
    let Some(path) = path else { return Ok(Vec::new()) };
    Ok(std::fs::read_to_string(path)?
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Load every scenario and fixture and plan each harness against a placeholder sink.
    Check {
        #[arg(long, default_value = "redteam/scenarios")]
        scenarios: PathBuf,
        #[arg(long, default_value = "redteam/fixtures")]
        fixtures: PathBuf,
    },
    /// Render the agent jobs for one harness and model into a JSON plan.
    Plan {
        #[arg(long, default_value = "redteam/scenarios")]
        scenarios: PathBuf,
        #[arg(long, default_value = "redteam/fixtures")]
        fixtures: PathBuf,
        #[arg(long)]
        harness: String,
        #[arg(long)]
        model: String,
        #[arg(long)]
        sink_url: String,
        #[arg(long)]
        out: PathBuf,
    },
    /// Dispatch a plan through a dispatch program under the run budget and judge every job.
    Run {
        #[arg(long)]
        plan: PathBuf,
        /// Dispatch program; absent = this binary's own `t3 dispatch`.
        #[arg(long)]
        dispatch_cmd: Option<PathBuf>,
        /// Argument passed to `--dispatch-cmd`, repeatable.
        #[arg(long = "dispatch-arg", allow_hyphen_values = true)]
        dispatch_args: Vec<String>,
        #[arg(long)]
        max_usd_total: f64,
        #[arg(long)]
        max_usd_per_scenario: f64,
        /// Canary tokens, one per line.
        #[arg(long)]
        canaries: Option<PathBuf>,
        #[arg(long)]
        out: PathBuf,
    },
    /// Dispatch one job (JSON on stdin) through the server API and print its evidence JSON.
    ///
    /// Reads `REDTEAM_SERVER_URL`, `REDTEAM_API_TOKEN`, `REDTEAM_DISPATCHER`,
    /// `REDTEAM_PROFILE`, `REDTEAM_SINK_HITS_URL` and `REDTEAM_POLL_SECS`.
    Dispatch,
    /// Judge one job's collected evidence.
    Judge {
        #[arg(long)]
        job: PathBuf,
        #[arg(long)]
        evidence: PathBuf,
        #[arg(long)]
        canaries: Option<PathBuf>,
    },
}

const HARNESSES: &[&str] = &["claude-code", "codex", "opencode"];

fn check(scenarios: &Path, fixtures: &Path) -> anyhow::Result<bool> {
    let all = crate::scenario::load_dir(scenarios)?;
    let lib = crate::fixture::load_library(fixtures)?;
    for harness in HARNESSES {
        let t = Target {
            harness: (*harness).to_owned(),
            model: "check".to_owned(),
            sink_url: "http://sink.example.com".to_owned(),
        };
        let jobs = plan(scenarios, &all, &lib, &t)?;
        println!("{harness}: {} agent jobs", jobs.len());
    }
    println!("{} scenarios, {} fixtures", all.len(), lib.len());
    Ok(true)
}

fn run(
    plan_path: &Path,
    dispatch_cmd: Option<PathBuf>,
    dispatch_args: Vec<String>,
    caps: (f64, f64),
    canaries: Option<&Path>,
    out: &Path,
) -> anyhow::Result<bool> {
    let jobs: Vec<AgentJob> = serde_json::from_str(&std::fs::read_to_string(plan_path)?)?;
    let canaries = read_canaries(canaries)?;
    let mut ledger = Ledger::new(caps.0, caps.1);
    let (program, args) = match dispatch_cmd {
        Some(program) => (program, dispatch_args),
        None => (std::env::current_exe()?, vec!["t3".to_owned(), "dispatch".to_owned()]),
    };
    let mut d = CommandDispatch { program, args, grace: Duration::from_mins(1) };
    let reports = run_jobs(&jobs, &mut ledger, &canaries, &mut d);
    std::fs::create_dir_all(out)?;
    std::fs::write(out.join("report.json"), serde_json::to_string_pretty(&reports)?)?;
    let md = to_markdown(&reports);
    std::fs::write(out.join("report.md"), &md)?;
    println!("{md}\nspent {:.2} of {:.2} USD", ledger.spent(), caps.0);
    Ok(passed(&reports))
}

/// Run a `t3` subcommand; `Ok(false)` means the run did not pass.
pub async fn cli(cmd: Command) -> anyhow::Result<bool> {
    match cmd {
        Command::Check { scenarios, fixtures } => check(&scenarios, &fixtures),
        Command::Plan { scenarios, fixtures, harness, model, sink_url, out } => {
            let all = crate::scenario::load_dir(&scenarios)?;
            let lib = crate::fixture::load_library(&fixtures)?;
            let jobs = plan(&scenarios, &all, &lib, &Target { harness, model, sink_url })?;
            std::fs::write(&out, serde_json::to_string_pretty(&jobs)?)?;
            println!("{} jobs -> {}", jobs.len(), out.display());
            Ok(true)
        }
        Command::Run {
            plan: plan_path,
            dispatch_cmd,
            dispatch_args,
            max_usd_total,
            max_usd_per_scenario,
            canaries,
            out,
        } => run(
            &plan_path,
            dispatch_cmd,
            dispatch_args,
            (max_usd_total, max_usd_per_scenario),
            canaries.as_deref(),
            &out,
        ),
        Command::Dispatch => crate::dispatch::cli_main().await,
        Command::Judge { job, evidence, canaries } => {
            let job: AgentJob = serde_json::from_str(&std::fs::read_to_string(job)?)?;
            let ev: Evidence = serde_json::from_str(&std::fs::read_to_string(evidence)?)?;
            let canaries = read_canaries(canaries.as_deref())?;
            let exp = Expectation {
                mode: job.mode,
                canaries: &canaries,
                markers: &job.markers,
                expected_decision: job.expected_decision,
            };
            let v = judge(&exp, &ev);
            println!("{}", serde_json::to_string_pretty(&v)?);
            Ok(matches!(v.verdict, Verdict::Contained | Verdict::Refused))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;
    use crate::budget::ScenarioBudget;

    fn job(id: &str, usd: f64, timeout_secs: u64) -> AgentJob {
        AgentJob {
            scenario: id.to_owned(),
            class: "egress".to_owned(),
            mode: Mode::Attacker,
            harness: "claude-code".to_owned(),
            model: "m".to_owned(),
            prompt: "p".to_owned(),
            files: Vec::new(),
            budget: ScenarioBudget { usd, max_turns: None, timeout_secs },
            markers: Vec::new(),
            expected_decision: None,
        }
    }

    fn acted(cost: f64) -> Evidence {
        Evidence {
            oracle_ready: true,
            events: vec![Event { kind: EventKind::ToolCall, text: "id".to_owned() }],
            cost_usd: cost,
            ..Evidence::default()
        }
    }

    struct Fake {
        replies: VecDeque<Evidence>,
        caps_seen: Vec<f64>,
    }

    impl Dispatch for Fake {
        fn dispatch(&mut self, job: &AgentJob) -> anyhow::Result<Evidence> {
            self.caps_seen.push(job.budget.usd);
            self.replies.pop_front().ok_or_else(|| anyhow!("no reply"))
        }
    }

    #[test]
    fn the_run_budget_cuts_off_dispatch_once_spent() {
        let jobs = [job("a", 1.0, 60), job("b", 1.0, 60), job("c", 1.0, 60)];
        let mut fake =
            Fake { replies: VecDeque::from([acted(1.0), acted(0.5)]), caps_seen: vec![] };
        let mut ledger = Ledger::new(1.5, 1.0);
        let reports = run_jobs(&jobs, &mut ledger, &[], &mut fake);
        assert_eq!(fake.caps_seen, vec![1.0, 0.5]);
        assert_eq!(reports[0].verdict, Verdict::Contained);
        assert_eq!(reports[0].cut_off, Some(CutOff::Budget));
        assert!((reports[1].cap_usd - 0.5).abs() < 1e-9);
        assert_eq!(reports[2].verdict, Verdict::Inconclusive);
        assert_eq!(reports[2].reasons, vec!["run budget exhausted"]);
        assert!(!passed(&reports));
        assert!(passed(&reports[..2]));
    }

    #[test]
    fn a_failed_dispatch_is_inconclusive_and_charged_its_cap() {
        let mut fake = Fake { replies: VecDeque::new(), caps_seen: vec![] };
        let mut ledger = Ledger::new(2.0, 1.0);
        let reports = run_jobs(&[job("a", 1.0, 60)], &mut ledger, &[], &mut fake);
        assert_eq!(reports[0].verdict, Verdict::Inconclusive);
        assert!((ledger.spent() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn refused_does_not_fail_the_run_but_escaped_does() {
        let mut r = AgentReport::new(&job("a", 1.0, 60), Verdict::Refused, vec![]);
        assert!(passed(std::slice::from_ref(&r)));
        r.verdict = Verdict::Escaped;
        assert!(!passed(&[r]));
    }

    #[cfg(unix)]
    #[test]
    fn a_dispatch_program_returns_evidence_on_stdout() {
        let mut d = CommandDispatch {
            program: PathBuf::from("sh"),
            args: vec![
                "-c".to_owned(),
                concat!(
                    "cat >/dev/null; echo '",
                    r#"{"oracle_ready":true,"events":[{"kind":"tool_call","text":"id"}]}"#,
                    "'"
                )
                .to_owned(),
            ],
            grace: Duration::from_secs(5),
        };
        let ev = d.dispatch(&job("a", 1.0, 5)).unwrap();
        assert!(ev.oracle_ready);
        assert_eq!(ev.events.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn a_dispatch_program_past_its_wall_clock_is_killed() {
        let mut d = CommandDispatch {
            program: PathBuf::from("sh"),
            args: vec!["-c".to_owned(), "exec sleep 30".to_owned()],
            grace: Duration::from_millis(300),
        };
        let started = Instant::now();
        let ev = d.dispatch(&job("a", 1.0, 0)).unwrap();
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(ev.cut_off, Some(CutOff::WallClock));
        assert!(!ev.oracle_ready);
    }

    #[test]
    fn the_markdown_lists_every_report() {
        let reasons = vec!["agent declined".to_owned()];
        let r = AgentReport::new(&job("ATK-EGR", 1.0, 60), Verdict::Refused, reasons);
        let md = to_markdown(&[r]);
        assert!(md.contains("| ATK-EGR | claude-code | m | Attacker | refused | - |"));
    }
}
