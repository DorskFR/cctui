//! Codex-native dispatch runner (CCT-643).
//!
//! A dispatched worker whose payload selects `adapter = "codex"` runs its task
//! headlessly through `codex exec --json` instead of the claude-code control
//! socket. This runner is deliberately SEPARATE from the interactive Rust
//! app-server adapter in `adapters/codex/` — that adapter drives long-lived,
//! attachable Codex threads; a dispatch is a one-shot, fire-and-report job.
//!
//! ## Why `codex exec`, not the Python Codex SDK
//!
//! See `docs/worker-contract.md` (§ "Codex-native dispatch") for the full spike
//! rationale. In short: the claude dispatch path shells out to a CLI
//! (`claude` via the `claude daemon` control socket), so shelling out to
//! `codex exec` keeps the two runners symmetric, adds no Python runtime to the
//! worker image, and reuses the per-pod `~/.codex/config.toml` the entrypoint
//! already hardens (approvals off, sandbox full-access, cctui gateway provider).
//! The SDK would pin its own Codex runtime and demand a Python layer for no gain.
//!
//! The runner parses the `codex exec --json` JSONL event stream into the same
//! `RESULT_FILE` verdict the claude path's callback trap consumes, so the
//! worker entrypoint's result-callback machinery is unchanged.

use std::collections::BTreeMap;
use std::process::Stdio;

use anyhow::Context;
use serde_json::{Value, json};

/// Default result-file path the worker entrypoint's callback trap reads
/// (`RESULT_FILE`, matching `deploy/worker-entrypoint.sh`).
const DEFAULT_RESULT_FILE: &str = "/tmp/cctui-result.json";

/// Adapter selectors that route a dispatch through this codex runner. Everything
/// else (absent, `claude`, `claude-code`) stays on the claude-code path.
#[must_use]
pub fn is_codex_adapter(adapter: &str) -> bool {
    matches!(adapter.trim(), "codex" | "codex-cli")
}

/// The adapter string a dispatch payload selects, defaulting to `claude-code`
/// so a payload with no `adapter` key is backward-compatible.
#[must_use]
pub fn payload_adapter(payload: &Value) -> String {
    payload
        .get("adapter")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("claude-code")
        .to_owned()
}

/// A parsed codex dispatch request: everything the runner needs to build and
/// run one `codex exec` invocation and report its verdict.
#[derive(Debug, Clone)]
pub struct CodexDispatch {
    /// Task id echoed into the result envelope (from `payload.task_id`, else
    /// the `TASK_ID` env, else empty).
    pub task_id: String,
    /// Resolved prompt text (inline `prompt` or the contents of `prompt_file`).
    pub prompt: String,
    /// Working root handed to `codex exec -C` (`CCTUI_DISPATCH_WORKDIR`,
    /// default `/workspace`).
    pub workdir: String,
    /// Optional codex model (`payload.model`); `None` → config.toml's pin.
    pub model: Option<String>,
    /// Optional reasoning effort (`payload.effort`); `None` → config.toml's pin.
    pub effort: Option<String>,
    /// Absolute path `codex exec -o` writes the final agent message to; also the
    /// runtime fallback when the JSONL stream carried no `agent_message`.
    pub last_message_file: String,
    /// Where the verdict is written for the entrypoint callback trap.
    pub result_file: String,
    /// Hard wall-clock bound, seconds (`CODEX_TIMEOUT`, default 3h). The k8s Job
    /// `activeDeadlineSeconds` is the outer bound; this is the inner guard.
    pub timeout_secs: u64,
}

impl CodexDispatch {
    /// Build a runner from the dispatch payload. Resolves the prompt eagerly so
    /// a missing prompt fails fast (before any process is spawned).
    ///
    /// `resolve_prompt` is shared with the claude-code path
    /// ([`resolve_dispatch_prompt`]) so both adapters honour the same
    /// `prompt` / `prompt_file` + `CCTUI_DISPATCH_PROMPT_DIRS` contract.
    pub fn from_payload(payload: &Value) -> anyhow::Result<Self> {
        let prompt = resolve_dispatch_prompt(payload)?;
        let workdir =
            std::env::var("CCTUI_DISPATCH_WORKDIR").unwrap_or_else(|_| "/workspace".to_owned());
        let task_id = payload
            .get("task_id")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .or_else(|| std::env::var("TASK_ID").ok())
            .filter(|s| !s.is_empty())
            .unwrap_or_default();
        let model = payload
            .get("model")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned);
        let effort = payload
            .get("effort")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned);
        let result_file =
            std::env::var("RESULT_FILE").unwrap_or_else(|_| DEFAULT_RESULT_FILE.to_owned());
        let timeout_secs = std::env::var("CODEX_TIMEOUT")
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .filter(|&s| s > 0)
            .unwrap_or(10_800);
        Ok(Self {
            task_id,
            prompt,
            workdir,
            model,
            effort,
            last_message_file: "/tmp/cctui-codex-last-message.txt".to_owned(),
            result_file,
            timeout_secs,
        })
    }

    /// The `codex exec` argument vector (everything after the `codex` program).
    ///
    /// Mirrors `deploy/codex-run.sh` (skip-git-repo-check, stdin closed at spawn)
    /// plus `--json` for a parseable event stream and `-o` for a robust last-
    /// message capture. Model/effort are passed EXPLICITLY (not left to
    /// config.toml) because a codex dispatch selects them per-request in the
    /// payload; approvals/sandbox/provider still come from the hardened
    /// per-pod config.toml.
    #[must_use]
    pub fn build_argv(&self) -> Vec<String> {
        let mut argv = vec![
            "exec".to_owned(),
            "--json".to_owned(),
            "--skip-git-repo-check".to_owned(),
            "-C".to_owned(),
            self.workdir.clone(),
            "-o".to_owned(),
            self.last_message_file.clone(),
        ];
        if let Some(model) = &self.model {
            argv.push("-m".to_owned());
            argv.push(model.clone());
        }
        if let Some(effort) = &self.effort {
            argv.push("-c".to_owned());
            argv.push(format!("model_reasoning_effort=\"{effort}\""));
        }
        // Prompt last, as a positional arg (stdin is closed at spawn).
        argv.push(self.prompt.clone());
        argv
    }

    /// Run `codex exec` to completion and write the verdict to `result_file`.
    /// Best-effort: every failure path still writes a `failed` result so the
    /// entrypoint callback trap has a valid envelope to POST.
    pub async fn run(&self) -> anyhow::Result<()> {
        let argv = self.build_argv();
        tracing::info!(
            task_id = %self.task_id,
            model = ?self.model,
            effort = ?self.effort,
            "codex dispatch: launching `codex exec`"
        );
        let mut cmd = tokio::process::Command::new("codex");
        cmd.args(&argv)
            .current_dir(&self.workdir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let outcome = match self.run_inner(cmd).await {
            Ok(outcome) => outcome,
            Err(err) => {
                tracing::error!(%err, "codex dispatch: run failed");
                CodexOutcome::failed(format!("codex exec failed to run: {err}"))
            }
        };
        let result = self.result_json(&outcome);
        let body = serde_json::to_string(&result).unwrap_or_else(|_| "{}".to_owned());
        std::fs::write(&self.result_file, &body)
            .with_context(|| format!("writing result file {}", self.result_file))?;
        tracing::info!(
            task_id = %self.task_id,
            status = %outcome.status,
            "codex dispatch: wrote {}",
            self.result_file
        );
        Ok(())
    }

    async fn run_inner(&self, mut cmd: tokio::process::Command) -> anyhow::Result<CodexOutcome> {
        let child = cmd.spawn().context("spawning codex exec")?;
        let fut = child.wait_with_output();
        let output = match tokio::time::timeout(
            std::time::Duration::from_secs(self.timeout_secs),
            fut,
        )
        .await
        {
            Ok(res) => res.context("awaiting codex exec")?,
            Err(_) => {
                return Ok(CodexOutcome::failed(format!(
                    "codex exec exceeded {}s timeout",
                    self.timeout_secs
                )));
            }
        };
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut outcome = parse_events(&stdout);
        // Fall back to codex's own last-message file when the stream carried no
        // agent_message (e.g. output buffering): it is the authoritative final
        // answer codex `-o` writes.
        if outcome.message.is_empty()
            && let Ok(text) = std::fs::read_to_string(&self.last_message_file)
            && !text.trim().is_empty()
        {
            text.trim().clone_into(&mut outcome.message);
        }
        if !output.status.success() && outcome.status == "success" {
            let stderr = String::from_utf8_lossy(&output.stderr);
            outcome = CodexOutcome::failed(format!(
                "codex exec exited with {}: {}",
                output.status,
                stderr.trim()
            ));
        }
        Ok(outcome)
    }

    /// The `RESULT_FILE` verdict envelope, matching the tenant-visible result
    /// contract (`docs/worker-contract.md` § "Result callback"): `task_id`,
    /// `status`, `error`, plus the codex final `message` and token `usage`.
    #[must_use]
    pub fn result_json(&self, outcome: &CodexOutcome) -> Value {
        let mut obj = serde_json::Map::new();
        obj.insert("task_id".to_owned(), json!(self.task_id));
        obj.insert("flow".to_owned(), json!("codex-dispatch"));
        obj.insert("status".to_owned(), json!(outcome.status));
        obj.insert("error".to_owned(), outcome.error.as_ref().map_or(Value::Null, |e| json!(e)));
        if !outcome.message.is_empty() {
            obj.insert("message".to_owned(), json!(outcome.message));
        }
        if let Some(usage) = &outcome.usage {
            obj.insert("usage".to_owned(), usage.clone());
        }
        Value::Object(obj)
    }
}

/// The distilled result of one `codex exec` run.
#[derive(Debug, Clone)]
pub struct CodexOutcome {
    /// `success` | `failed`, matching the result-callback contract.
    pub status: String,
    /// One-line reason when `status != success`.
    pub error: Option<String>,
    /// The final agent message text.
    pub message: String,
    /// The `turn.completed` token usage object, if the stream carried one.
    pub usage: Option<Value>,
}

impl CodexOutcome {
    fn failed(reason: impl Into<String>) -> Self {
        Self {
            status: "failed".to_owned(),
            error: Some(reason.into()),
            message: String::new(),
            usage: None,
        }
    }
}

/// Parse a `codex exec --json` JSONL stream into a [`CodexOutcome`].
///
/// The event envelope (codex 0.144.x) is a flat `{"type": "...", ...}` per line:
///
///   * `item.completed` with `item.type == "agent_message"` → final answer text
///     (last one wins).
///   * `turn.completed` → carries the `usage` token counts.
///   * `error` / `turn.failed` → mark the run failed with the message.
///
/// Unparseable / unknown lines are ignored so a codex event-schema addition
/// never breaks the runner.
#[must_use]
pub fn parse_events(stdout: &str) -> CodexOutcome {
    let mut message = String::new();
    let mut usage: Option<Value> = None;
    let mut error: Option<String> = None;
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(ev) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match ev.get("type").and_then(Value::as_str) {
            Some("item.completed") => {
                let item = ev.get("item");
                let is_msg = item
                    .and_then(|i| i.get("type"))
                    .and_then(Value::as_str)
                    .is_some_and(|t| t == "agent_message" || t == "assistant_message");
                if is_msg
                    && let Some(text) = item.and_then(|i| i.get("text")).and_then(Value::as_str)
                    && !text.is_empty()
                {
                    text.clone_into(&mut message);
                }
            }
            Some("turn.completed") => {
                if let Some(u) = ev.get("usage") {
                    usage = Some(u.clone());
                }
            }
            Some("error") => {
                error = Some(
                    ev.get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("codex reported an error")
                        .to_owned(),
                );
            }
            Some("turn.failed") => {
                error = Some(
                    ev.get("error")
                        .and_then(|e| e.get("message"))
                        .and_then(Value::as_str)
                        .unwrap_or("codex turn failed")
                        .to_owned(),
                );
            }
            _ => {}
        }
    }
    match error {
        Some(reason) => {
            CodexOutcome { status: "failed".to_owned(), error: Some(reason), message, usage }
        }
        None => CodexOutcome { status: "success".to_owned(), error: None, message, usage },
    }
}

/// Resolve the dispatched prompt from the payload.
///
/// An inline `prompt` wins; else a `prompt_file` is searched across
/// `CCTUI_DISPATCH_PROMPT_DIRS` (default `/opt/context/prompts:/prompts`), with
/// an absolute `prompt_file` read as-is. Shared with the claude-code dispatch
/// path so both adapters resolve the prompt identically.
pub fn resolve_dispatch_prompt(payload: &Value) -> anyhow::Result<String> {
    if let Some(p) = payload.get("prompt").and_then(Value::as_str)
        && !p.is_empty()
    {
        return Ok(p.to_owned());
    }
    let file =
        payload.get("prompt_file").and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or_else(
            || anyhow::anyhow!("dispatch payload has neither prompt nor prompt_file"),
        )?;
    if file.starts_with('/') {
        return std::fs::read_to_string(file)
            .with_context(|| format!("reading prompt file {file}"));
    }
    let dirs = std::env::var("CCTUI_DISPATCH_PROMPT_DIRS")
        .unwrap_or_else(|_| "/opt/context/prompts:/prompts".to_owned());
    for dir in dirs.split(':').filter(|d| !d.is_empty()) {
        let path = std::path::Path::new(dir).join(file);
        if path.is_file() {
            return std::fs::read_to_string(&path)
                .with_context(|| format!("reading prompt file {}", path.display()));
        }
    }
    anyhow::bail!("prompt_file {file} not found under {dirs}")
}

/// Environment-map extraction shared with the claude path (unused fields dropped).
#[must_use]
pub fn payload_env(payload: &Value) -> BTreeMap<String, String> {
    payload
        .get("env")
        .and_then(Value::as_object)
        .map(|o| {
            o.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned()))).collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dispatch(prompt: &str) -> CodexDispatch {
        CodexDispatch {
            task_id: "T-1".to_owned(),
            prompt: prompt.to_owned(),
            workdir: "/workspace".to_owned(),
            model: None,
            effort: None,
            last_message_file: "/tmp/last.txt".to_owned(),
            result_file: "/tmp/cctui-result.json".to_owned(),
            timeout_secs: 500,
        }
    }

    #[test]
    fn adapter_selection() {
        assert!(is_codex_adapter("codex"));
        assert!(is_codex_adapter(" codex "));
        assert!(is_codex_adapter("codex-cli"));
        assert!(!is_codex_adapter("claude-code"));
        assert!(!is_codex_adapter("claude"));
        assert!(!is_codex_adapter(""));
    }

    #[test]
    fn payload_adapter_defaults_to_claude() {
        assert_eq!(payload_adapter(&json!({})), "claude-code");
        assert_eq!(payload_adapter(&json!({"adapter": ""})), "claude-code");
        assert_eq!(payload_adapter(&json!({"adapter": "codex"})), "codex");
        assert_eq!(payload_adapter(&json!({"adapter": " codex "})), "codex");
    }

    #[test]
    fn build_argv_minimal() {
        let argv = dispatch("do the thing").build_argv();
        assert_eq!(
            argv,
            vec![
                "exec",
                "--json",
                "--skip-git-repo-check",
                "-C",
                "/workspace",
                "-o",
                "/tmp/last.txt",
                "do the thing",
            ]
        );
    }

    #[test]
    fn build_argv_with_model_and_effort() {
        let mut d = dispatch("go");
        d.model = Some("gpt-5.6-sol".to_owned());
        d.effort = Some("high".to_owned());
        let argv = d.build_argv();
        // model + effort injected, prompt stays last.
        let joined = argv.join(" ");
        assert!(joined.contains("-m gpt-5.6-sol"));
        assert!(joined.contains("-c model_reasoning_effort=\"high\""));
        assert_eq!(argv.last().unwrap(), "go");
        // Positional prompt is never confused for a flag value.
        assert_eq!(argv[0], "exec");
    }

    #[test]
    fn from_payload_resolves_inline_prompt() {
        let d = CodexDispatch::from_payload(&json!({
            "adapter": "codex",
            "prompt": "hello",
            "model": "gpt-5.6-terra",
            "effort": "low",
            "task_id": "CCT-1",
        }))
        .unwrap();
        assert_eq!(d.prompt, "hello");
        assert_eq!(d.model.as_deref(), Some("gpt-5.6-terra"));
        assert_eq!(d.effort.as_deref(), Some("low"));
        assert_eq!(d.task_id, "CCT-1");
    }

    #[test]
    fn from_payload_errors_without_prompt() {
        assert!(CodexDispatch::from_payload(&json!({"adapter": "codex"})).is_err());
    }

    #[test]
    fn parse_events_success_extracts_message_and_usage() {
        let stream = r#"
{"type":"thread.started","thread_id":"019f52d5"}
{"type":"turn.started"}
{"type":"item.started","item":{"id":"m1","type":"agent_message","text":""}}
{"type":"item.completed","item":{"id":"m1","type":"agent_message","text":"All done."}}
{"type":"turn.completed","usage":{"input_tokens":10,"output_tokens":5}}
"#;
        let outcome = parse_events(stream);
        assert_eq!(outcome.status, "success");
        assert_eq!(outcome.message, "All done.");
        assert!(outcome.error.is_none());
        assert_eq!(outcome.usage.unwrap()["output_tokens"], json!(5));
    }

    #[test]
    fn parse_events_last_agent_message_wins() {
        let stream = concat!(
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"first\"}}\n",
            "{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"final\"}}\n",
        );
        assert_eq!(parse_events(stream).message, "final");
    }

    #[test]
    fn parse_events_turn_failed_is_failure() {
        let stream = concat!(
            "{\"type\":\"turn.started\"}\n",
            "{\"type\":\"error\",\"message\":\"You hit your spend cap.\"}\n",
            "{\"type\":\"turn.failed\",\"error\":{\"message\":\"You hit your spend cap.\"}}\n",
        );
        let outcome = parse_events(stream);
        assert_eq!(outcome.status, "failed");
        assert_eq!(outcome.error.as_deref(), Some("You hit your spend cap."));
    }

    #[test]
    fn parse_events_ignores_garbage_lines() {
        let stream = "not json\n{\"type\":\"turn.completed\",\"usage\":{}}\n\n";
        let outcome = parse_events(stream);
        assert_eq!(outcome.status, "success");
    }

    #[test]
    fn result_json_shape_success() {
        let d = dispatch("x");
        let outcome = CodexOutcome {
            status: "success".to_owned(),
            error: None,
            message: "done".to_owned(),
            usage: Some(json!({"input_tokens": 1})),
        };
        let r = d.result_json(&outcome);
        assert_eq!(r["task_id"], json!("T-1"));
        assert_eq!(r["status"], json!("success"));
        assert_eq!(r["error"], Value::Null);
        assert_eq!(r["message"], json!("done"));
        assert_eq!(r["flow"], json!("codex-dispatch"));
        assert_eq!(r["usage"]["input_tokens"], json!(1));
    }

    #[test]
    fn result_json_shape_failure() {
        let d = dispatch("x");
        let r = d.result_json(&CodexOutcome::failed("boom"));
        assert_eq!(r["status"], json!("failed"));
        assert_eq!(r["error"], json!("boom"));
        assert!(r.get("message").is_none());
    }
}
