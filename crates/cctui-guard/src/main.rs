//! `cctui-guard` — markdown-driven workflow guard daemon.
//!
//! Parses a prompt markdown file into steps and serves a localhost HTTP API
//! that Claude Code's `PreToolUse` hook calls before every tool invocation.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;

use cctui_guard::engine::WorkflowEngine;
use cctui_guard::parser::{parse_guard_rules_files, parse_steps};
use cctui_guard::server::router;

#[derive(Parser, Debug)]
#[command(name = "cctui-guard", about = "Markdown-driven workflow guard daemon")]
struct Cli {
    /// Prompt markdown file containing the `# Step N` definitions.
    #[arg(long, env = "PROMPT_FILE")]
    prompt: PathBuf,

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

    let markdown = std::fs::read_to_string(&cli.prompt)
        .map_err(|e| anyhow::anyhow!("prompt file {}: {e}", cli.prompt.display()))?;
    let steps = parse_steps(&markdown)
        .map_err(|e| anyhow::anyhow!("prompt file {}: {e}", cli.prompt.display()))?;
    if steps.is_empty() {
        tracing::warn!("No steps found in {}", cli.prompt.display());
    }

    // Layer the rules: operator base first (if any), then the (pack's) rules
    // file — `[name]:` overrides, `[name]+:` extends. parse_guard_rules_files
    // skips missing layers, so an absent base or rules file is not fatal.
    let mut layers: Vec<PathBuf> = Vec::new();
    if let Some(base) = &cli.rules_base {
        if base.exists() {
            layers.push(base.clone());
        } else {
            tracing::warn!("Guard rules base not found: {}", base.display());
        }
    }
    if cli.rules.exists() {
        layers.push(cli.rules.clone());
    } else {
        tracing::warn!("Guard rules file not found: {}", cli.rules.display());
    }
    let tool_sets = parse_guard_rules_files(&layers)?;

    tracing::info!(
        "Loaded {} steps, {} tool sets from {} rule layer(s)",
        steps.len(),
        tool_sets.len(),
        layers.len()
    );

    let engine = Arc::new(WorkflowEngine::new(
        steps,
        tool_sets,
        cli.state,
        cli.policy_out,
        cli.always_allow,
        cli.gate_cwd,
        cli.judge_cmd,
    ));

    let app = router(engine);
    let listener = tokio::net::TcpListener::bind(cli.listen).await?;
    tracing::info!("Listening on {}", cli.listen);
    axum::serve(listener, app).await?;
    Ok(())
}
