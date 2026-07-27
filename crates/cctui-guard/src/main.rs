//! `cctui-guard` — markdown-driven workflow guard daemon.
//!
//! Parses a prompt markdown file into steps and serves a localhost HTTP API
//! that Claude Code's `PreToolUse` hook calls before every tool invocation.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::{Parser, Subcommand};

use cctui_guard::engine::WorkflowEngine;
use cctui_guard::ir::{NetworkDefault, Workflow};
use cctui_guard::lint::{Diagnostic, LintReport, Severity};
use cctui_guard::parser::step_heading_numbers;
use cctui_guard::resolve::{ResolvedSets, resolve_sets};
use cctui_guard::server::router;

#[derive(Parser, Debug)]
#[command(name = "cctui-guard", about = "Markdown-driven workflow guard daemon")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Prompt file containing the workflow: markdown with `# Step N`
    /// definitions, or — for machine writers — a `workflow.json` matching the
    /// published IR schema (detected by a `.json` extension). Not required when
    /// `--emit-schema` is set or a subcommand is given.
    #[arg(long, env = "PROMPT_FILE")]
    prompt: Option<PathBuf>,

    /// Print the published JSON Schema for the workflow IR to stdout and exit.
    #[arg(long = "emit-schema")]
    emit_schema: bool,

    /// Lint the prompt + rules before serving; refuse to start on any error.
    #[arg(long)]
    check: bool,

    /// Operator base guard-rules file, parsed **before** `--rules` so a context
    /// pack can reuse/extend/override its sets (common definitions like
    /// `net-dev` live here). Optional — skipped if it does not exist.
    #[arg(long = "rules-base", env = "GUARD_RULES_BASE")]
    rules_base: Option<PathBuf>,

    /// Shared guard-rules file defining tool sets and network sets. Parsed after
    /// `--rules-base`; `[name]:` overrides a base set, `[name]+:` extends it.
    #[arg(long, env = "GUARD_RULES_FILE", default_value = "/etc/claude-worker/guard-rules.md")]
    rules: PathBuf,

    /// Address to listen on.
    #[arg(long, default_value = "127.0.0.1:9999")]
    listen: SocketAddr,

    /// Root-owned state file.
    #[arg(long, default_value = "/var/run/workflow-guard/state")]
    state: PathBuf,

    /// Guard-proxy policy output file.
    #[arg(long = "policy-out", default_value = "/var/run/guard-proxy/policy.json")]
    policy_out: PathBuf,

    /// Hosts always allowed in every written policy (repeatable), e.g.
    /// `--always-allow automation.example.com:443`.
    #[arg(long = "always-allow")]
    always_allow: Vec<String>,

    /// Working directory the deterministic transition `[gate]` command runs in
    /// (the worker's task tree).
    #[arg(long = "gate-cwd", default_value = "/workspace")]
    gate_cwd: PathBuf,

    /// Command the `[llmjudge]` acceptance judge runs through (CCT-516). Runs
    /// via `sh -c` in `--gate-cwd`, receives the question prompt on stdin, and
    /// must print a JSON verdict array on stdout (e.g. a wrapper around
    /// `claude -p` with a clean context). Unset while a step declares
    /// `[llmjudge]` ⇒ that step's transition is refused (fail closed).
    #[arg(long = "judge-cmd", env = "GUARD_JUDGE_CMD")]
    judge_cmd: Option<String>,

    /// Append every `/check` and `/transition` decision as a JSON line here. The
    /// guard-proxy points its own `--decision-log` at the same file so egress
    /// verdicts land in one timeline. Unset ⇒ no log.
    #[arg(long = "decision-log", env = "GUARD_DECISION_LOG")]
    decision_log: Option<PathBuf>,

    /// Where the end-of-run report (aggregated from the decision log) is written
    /// on Exit. Requires `--decision-log`.
    #[arg(long = "report-out", env = "GUARD_REPORT_OUT")]
    report_out: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Validate a prompt + rules and dump the resolved policy, without serving.
    Lint(LintArgs),
}

#[derive(Parser, Debug)]
struct LintArgs {
    /// Prompt markdown (or `workflow.json`) to validate.
    prompt: PathBuf,

    /// Operator base guard-rules layer, parsed before `--rules`.
    #[arg(long = "rules-base", env = "GUARD_RULES_BASE")]
    rules_base: Option<PathBuf>,

    /// Shared guard-rules file defining tool sets and network sets.
    #[arg(long, env = "GUARD_RULES_FILE", default_value = "/etc/claude-worker/guard-rules.md")]
    rules: PathBuf,

    /// Print each step's resolved policy — sets expanded to concrete hosts and
    /// command phrases.
    #[arg(long)]
    explain: bool,
}

/// Load a workflow from either frontend: a `.json` file deserializes straight
/// into the IR (machine writers), anything else is compiled from prompt
/// markdown. Both produce the same [`Workflow`]. The second element is every
/// step id in authoring order (duplicates preserved) so the linter can flag a
/// repeated step number the step map would collapse.
fn load_workflow(path: &Path) -> anyhow::Result<(Workflow, Vec<u32>)> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("prompt file {}: {e}", path.display()))?;
    if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("json")) {
        let workflow = Workflow::from_json(&text)
            .map_err(|e| anyhow::anyhow!("workflow json {}: {e}", path.display()))?;
        let ids = workflow.steps.iter().map(|s| s.id).collect();
        Ok((workflow, ids))
    } else {
        let workflow = Workflow::compile(&text)
            .map_err(|e| anyhow::anyhow!("prompt file {}: {e}", path.display()))?;
        Ok((workflow, step_heading_numbers(&text)))
    }
}

/// Push an [`Severity::Error`] diagnostic for each unreadable `[rules]` import so
/// a broken prompt-declared dependency fails the lint like any policy error.
fn add_import_errors(report: &mut LintReport, import_errors: &[String]) {
    for message in import_errors {
        report.diagnostics.push(Diagnostic {
            severity: Severity::Error,
            step: None,
            message: message.clone(),
        });
    }
}

/// Print each resolved set's provenance — which layer (CLI file, `[rules]`
/// import, or inline prompt) last defined it — under `--explain`.
fn print_provenance(resolved: &ResolvedSets) {
    if resolved.provenance.is_empty() {
        return;
    }
    println!("\nSet provenance (effective source of each set):");
    for (name, source) in &resolved.provenance {
        println!("  {name}: {source}");
    }
}

/// Print the report to stderr; with `explain`, also dump each step's resolved
/// policy to stdout. Returns whether the lint passed (no errors).
fn print_lint(report: &LintReport, explain: bool) -> bool {
    for diag in &report.diagnostics {
        eprintln!("{diag}");
    }
    let (errors, warnings) = report.diagnostics.iter().fold((0, 0), |(e, w), d| match d.severity {
        Severity::Error => (e + 1, w),
        Severity::Warning => (e, w + 1),
    });

    if explain {
        for step in &report.resolved {
            println!("\nStep {}: {}", step.id, step.title);
            println!("  allowed:    {}", fmt_list(&step.allowed, "(unrestricted)"));
            println!("  disallowed: {}", fmt_list(&step.disallowed, "(none)"));
            let net = if step.network_open {
                "* (open)".to_string()
            } else {
                fmt_list(&step.network, "(deny — no egress)")
            };
            println!("  network:    {net}");
            let mut targets: Vec<String> = step.transitions.iter().map(u32::to_string).collect();
            if step.exit {
                targets.push("Exit".to_string());
            }
            println!("  transition: {}", fmt_list(&targets, "(dead end)"));
            if step.gate {
                println!("  gate:       yes");
            }
            if !step.transition_gates.is_empty() {
                let mut targets: Vec<u32> = step.transition_gates.clone();
                targets.sort_unstable();
                let targets: Vec<String> = targets.iter().map(u32::to_string).collect();
                println!("  gate→step:  {}", targets.join(", "));
            }
            if let Some(max) = step.max_visits {
                println!("  max-visits: {max}");
            }
            if step.judge > 0 {
                println!("  llmjudge:   {} question(s)", step.judge);
            }
        }
    }

    if report.has_errors() {
        eprintln!("lint failed: {errors} error(s), {warnings} warning(s)");
        false
    } else {
        eprintln!("lint passed: {warnings} warning(s)");
        true
    }
}

fn fmt_list(items: &[String], empty: &str) -> String {
    if items.is_empty() { empty.to_string() } else { items.join(", ") }
}

fn run_lint(args: &LintArgs) -> anyhow::Result<bool> {
    let (workflow, ids) = load_workflow(&args.prompt)?;
    let resolved = resolve_sets(args.rules_base.as_deref(), &args.rules, &args.prompt, &workflow);
    let mut report = cctui_guard::lint::lint(&workflow, &resolved.sets, &ids);
    add_import_errors(&mut report, &resolved.import_errors);
    let ok = print_lint(&report, args.explain);
    if args.explain {
        print_provenance(&resolved);
    }
    Ok(ok)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    if let Some(Command::Lint(args)) = &cli.command {
        return if run_lint(args)? { Ok(()) } else { std::process::exit(1) };
    }

    if cli.emit_schema {
        println!("{}", serde_json::to_string_pretty(&cctui_guard::ir::json_schema())?);
        return Ok(());
    }

    let Some(prompt) = cli.prompt.clone() else {
        anyhow::bail!("--prompt is required (or pass --emit-schema / the `lint` subcommand)");
    };
    let (workflow, ids) = load_workflow(&prompt)?;
    let resolved = resolve_sets(cli.rules_base.as_deref(), &cli.rules, &prompt, &workflow);
    for message in &resolved.import_errors {
        tracing::error!("{message}");
    }

    if cli.check {
        let mut report = cctui_guard::lint::lint(&workflow, &resolved.sets, &ids);
        add_import_errors(&mut report, &resolved.import_errors);
        if !print_lint(&report, false) {
            anyhow::bail!("--check found policy errors; refusing to start");
        }
    } else if !resolved.import_errors.is_empty() {
        anyhow::bail!("unreadable [rules] import(s); refusing to start (fail closed)");
    }
    let tool_sets = resolved.sets;

    let guarded_default_allow = matches!(workflow.network_default, Some(NetworkDefault::Allow));
    if guarded_default_allow {
        for step in &workflow.steps {
            if step.network.is_empty() {
                tracing::warn!(
                    "Step {} has no [network] under [network-default]: allow — egress is \
                     silently open; add [network] or drop the document override",
                    step.id
                );
            }
        }
    }
    let steps = workflow.into_steps();
    if steps.is_empty() {
        tracing::warn!("No steps found in {}", prompt.display());
    }

    tracing::info!("Loaded {} steps, {} tool sets", steps.len(), tool_sets.len());

    let engine = Arc::new(WorkflowEngine::new_with_log(
        steps,
        tool_sets,
        cli.state,
        cli.policy_out,
        cli.always_allow,
        cli.gate_cwd,
        cli.judge_cmd,
        guarded_default_allow,
        cctui_guard::decision_log::DecisionLog::new(cli.decision_log),
        cli.report_out,
    ));

    let app = router(engine);
    let listener = tokio::net::TcpListener::bind(cli.listen).await?;
    tracing::info!("Listening on {}", cli.listen);
    axum::serve(listener, app).await?;
    Ok(())
}
