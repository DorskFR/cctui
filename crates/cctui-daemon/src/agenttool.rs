//! Daemon side of the `CctuiAgent` tool.
//!
//! Listens on a local Unix socket for the `cctui-daemon mcp-agent` relay, asks
//! the server to spawn the requested child (the server owns the capability
//! decision — the daemon never grants anything itself), then blocks on the
//! child's completion via [`crate::childwatch`] and writes the child's final
//! message back as the tool result.

use std::path::{Path, PathBuf};
use std::time::Duration;

use cctui_proto::api::SpawnChildRequest;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio_util::sync::CancellationToken;

use crate::client::ServerClient;

/// Socket the session's MCP relay connects to. Kept beside the daemon's other
/// runtime state so a worker container with an unwritable `~/.config` still
/// finds a usable path.
#[must_use]
pub fn socket_path() -> PathBuf {
    crate::runtime::state_candidates("cctui-agent.sock")
        .into_iter()
        .next()
        .unwrap_or_else(|| std::env::temp_dir().join("cctui-agent.sock"))
}

/// A validated tool call: the parsed request line off the socket.
struct Call {
    session_id: String,
    request: SpawnChildRequest,
    timeout: Duration,
}

/// Parse one socket line into a spawn request. Returns the error text to hand
/// back to the model when the line is unusable.
fn parse_call(line: &str) -> Result<Call, String> {
    let v: Value = serde_json::from_str(line).map_err(|e| format!("malformed request: {e}"))?;
    if v.get("kind").and_then(Value::as_str) != Some("spawn_agent") {
        return Err("unsupported request kind".to_owned());
    }
    let session_id = v
        .get("session_id")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or("request carries no session id")?
        .to_owned();
    let args = v.get("args").cloned().unwrap_or_else(|| json!({}));
    let adapter = normalize_adapter(args.get("adapter").and_then(Value::as_str).unwrap_or(""));
    let prompt = args.get("prompt").and_then(Value::as_str).unwrap_or("").to_owned();
    if prompt.trim().is_empty() {
        return Err("prompt is required".to_owned());
    }
    let request = SpawnChildRequest {
        adapter,
        prompt,
        model: string_arg(&args, "model"),
        agent_profile: string_arg(&args, "agent_profile"),
        budget_usd: args.get("budget_usd").and_then(Value::as_f64),
        cwd: string_arg(&args, "cwd"),
    };
    let timeout = crate::mcp::resolve_timeout(v.get("timeout_secs").and_then(Value::as_u64));
    Ok(Call { session_id, request, timeout })
}

fn string_arg(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Accept the model-facing spellings of an adapter id and return the canonical
/// one. `claude_code`/`claude` are the ids a model is most likely to guess.
#[must_use]
pub fn normalize_adapter(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().replace('_', "-").as_str() {
        "claude" | "claude-code" => "claude-code".to_owned(),
        "codex" | "codex-cli" => "codex".to_owned(),
        other => other.to_owned(),
    }
}

/// Render a finished child into the tool's reply frame.
fn reply_frame(outcome: &crate::childwatch::ChildOutcome) -> Value {
    match (&outcome.error, &outcome.final_text) {
        (Some(err), Some(text)) => {
            json!({ "ok": false, "error": format!("child agent failed: {err}\n\nlast output:\n{text}") })
        }
        (Some(err), None) => json!({ "ok": false, "error": format!("child agent failed: {err}") }),
        (None, Some(text)) => json!({ "ok": true, "result": text }),
        (None, None) => {
            json!({ "ok": true, "result": "child agent finished without producing any output" })
        }
    }
}

/// Run one call end to end: spawn through the server, then wait for the child.
async fn run_call(server: &ServerClient, machine_key: &str, call: Call) -> Value {
    let child = match server.spawn_child(machine_key, &call.session_id, &call.request).await {
        Ok(child) => child,
        Err(err) => return json!({ "ok": false, "error": err.to_string() }),
    };
    let watch = crate::childwatch::global();
    let done = watch.register(&child.session_id, &call.session_id);
    tracing::info!(
        parent = %call.session_id,
        child = %child.session_id,
        adapter = %call.request.adapter,
        "CctuiAgent waiting on child",
    );
    match tokio::time::timeout(call.timeout, done).await {
        Ok(Ok(outcome)) => reply_frame(&outcome),
        Ok(Err(_)) => json!({ "ok": false, "error": "child agent tracking was dropped" }),
        Err(_) => {
            watch.cancel(&child.session_id);
            json!({
                "ok": false,
                "error": format!(
                    "child agent {} did not finish within {}s — it may still be running; \
                     check the session in cctui",
                    child.session_id,
                    call.timeout.as_secs(),
                ),
            })
        }
    }
}

async fn handle_connection(stream: UnixStream, server: ServerClient, machine_key: String) {
    let (read_half, mut write_half) = stream.into_split();
    let mut lines = BufReader::new(read_half).lines();
    let Ok(Some(line)) = lines.next_line().await else { return };
    let frame = match parse_call(&line) {
        Ok(call) => run_call(&server, &machine_key, call).await,
        Err(err) => json!({ "ok": false, "error": err }),
    };
    let _ = write_half.write_all(format!("{frame}\n").as_bytes()).await;
    let _ = write_half.flush().await;
}

/// Serve the agent-tool socket until `shutdown`.
pub async fn serve(
    path: PathBuf,
    server: ServerClient,
    machine_key: String,
    shutdown: CancellationToken,
) -> anyhow::Result<()> {
    let _ = std::fs::remove_file(&path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let listener = UnixListener::bind(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    tracing::info!(socket = %path.display(), "CctuiAgent tool listener ready");
    loop {
        tokio::select! {
            () = shutdown.cancelled() => {
                let _ = std::fs::remove_file(&path);
                return Ok(());
            }
            accept = listener.accept() => {
                let (stream, _) = accept?;
                let server = server.clone();
                let machine_key = machine_key.clone();
                tokio::spawn(handle_connection(stream, server, machine_key));
            }
        }
    }
}

/// Whether the daemon can serve the tool at all: without a machine key there is
/// nobody to authorize a spawn against, so the listener stays off.
#[must_use]
pub fn is_available(machine_key: &str) -> bool {
    !machine_key.trim().is_empty()
}

/// Path used when writing a session's MCP config, exposed for the launch path.
#[must_use]
pub fn socket_for_launch() -> &'static Path {
    static PATH: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    PATH.get_or_init(socket_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::childwatch::ChildOutcome;

    #[test]
    fn parses_a_full_call() {
        let line = json!({
            "kind": "spawn_agent",
            "session_id": "parent-1",
            "timeout_secs": 120,
            "args": {
                "adapter": "opencode",
                "prompt": "review the diff",
                "model": "accounts/fireworks/models/kimi-k3",
                "agent_profile": "cctui-reviewer",
                "budget_usd": 0.5,
                "cwd": "/workspace",
            },
        })
        .to_string();
        let call = parse_call(&line).unwrap();
        assert_eq!(call.session_id, "parent-1");
        assert_eq!(call.timeout, Duration::from_mins(2));
        assert_eq!(call.request.adapter, "opencode");
        assert_eq!(call.request.agent_profile.as_deref(), Some("cctui-reviewer"));
        assert_eq!(call.request.budget_usd, Some(0.5));
        assert_eq!(call.request.cwd.as_deref(), Some("/workspace"));
    }

    #[test]
    fn blank_optional_args_are_dropped_not_forwarded_empty() {
        let line = json!({
            "kind": "spawn_agent",
            "session_id": "p",
            "args": { "adapter": "codex", "prompt": "go", "model": "  ", "cwd": "" },
        })
        .to_string();
        let call = parse_call(&line).unwrap();
        assert!(call.request.model.is_none());
        assert!(call.request.cwd.is_none());
    }

    #[test]
    fn a_call_without_a_prompt_or_session_is_rejected() {
        let no_prompt =
            json!({ "kind": "spawn_agent", "session_id": "p", "args": { "adapter": "codex" } });
        assert!(parse_call(&no_prompt.to_string()).is_err());
        let no_session = json!({ "kind": "spawn_agent", "args": { "prompt": "x" } });
        assert!(parse_call(&no_session.to_string()).is_err());
        assert!(parse_call("not json").is_err());
        assert!(parse_call(&json!({ "kind": "other" }).to_string()).is_err());
    }

    #[test]
    fn model_spellings_normalize_to_adapter_ids() {
        assert_eq!(normalize_adapter("claude_code"), "claude-code");
        assert_eq!(normalize_adapter("Claude"), "claude-code");
        assert_eq!(normalize_adapter("codex-cli"), "codex");
        assert_eq!(normalize_adapter(" opencode "), "opencode");
    }

    #[test]
    fn reply_frames_distinguish_success_failure_and_silence() {
        let ok =
            reply_frame(&ChildOutcome { final_text: Some("verdict: ship".into()), error: None });
        assert_eq!(ok["ok"], json!(true));
        assert_eq!(ok["result"], "verdict: ship");

        let failed = reply_frame(&ChildOutcome { final_text: None, error: Some("crashed".into()) });
        assert_eq!(failed["ok"], json!(false));
        assert!(failed["error"].as_str().unwrap().contains("crashed"));

        let partial = reply_frame(&ChildOutcome {
            final_text: Some("got halfway".into()),
            error: Some("killed".into()),
        });
        assert_eq!(partial["ok"], json!(false));
        let text = partial["error"].as_str().unwrap();
        assert!(text.contains("killed") && text.contains("got halfway"));

        let silent = reply_frame(&ChildOutcome { final_text: None, error: None });
        assert_eq!(silent["ok"], json!(true));
        assert!(silent["result"].as_str().unwrap().contains("without producing any output"));
    }

    #[test]
    fn tool_is_unavailable_without_a_machine_key() {
        assert!(!is_available(""));
        assert!(!is_available("   "));
        assert!(is_available("machine-key"));
    }
}
