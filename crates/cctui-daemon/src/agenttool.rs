//! Daemon side of the `CctuiAgent` tool.
//!
//! Listens on a local Unix socket for the `cctui-daemon mcp-agent` relay.
//! A call spawns a child through the server (the server owns the capability
//! decision — the daemon never grants anything itself), or, when it names a
//! `session_id`, sends a follow-up prompt into a child spawned earlier.
//! Both then follow the child via [`crate::childwatch`], streaming progress
//! frames to a proto≥2 relay while waiting and finishing with the child's
//! final message. Proto 1 relays (older, still attached to live sessions)
//! get the single final line only.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cctui_proto::api::{MessageChildRequest, SpawnChildRequest};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio_util::sync::CancellationToken;

use crate::childwatch::{Assessment, WatchHandle, snippet};
use crate::client::ServerClient;

/// Cadence of progress frames to the relay while a child runs.
const PROGRESS_EVERY: Duration = Duration::from_secs(15);

/// A child that has shown no sign of life at all by this point never reached
/// its first model call — waiting out `timeout_secs` only delays the failure.
const SILENT_CHILD_GRACE: Duration = Duration::from_secs(90);

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

enum CallKind {
    Spawn(SpawnChildRequest),
    Message(MessageChildRequest),
}

struct Call {
    session_id: String,
    kind: CallKind,
    timeout: Duration,
    /// Relay protocol: ≥2 understands interim `progress` frames.
    proto: u64,
}

fn parse_call(line: &str) -> Result<Call, String> {
    let v: Value = serde_json::from_str(line).map_err(|e| format!("malformed request: {e}"))?;
    let session_id = v
        .get("session_id")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or("request carries no session id")?
        .to_owned();
    let args = v.get("args").cloned().unwrap_or_else(|| json!({}));
    let prompt = args.get("prompt").and_then(Value::as_str).unwrap_or("").to_owned();
    if prompt.trim().is_empty() {
        return Err("prompt is required".to_owned());
    }
    let timeout = crate::mcp::resolve_timeout(v.get("timeout_secs").and_then(Value::as_u64));
    let proto = v.get("proto").and_then(Value::as_u64).unwrap_or(1);
    let kind = match v.get("kind").and_then(Value::as_str) {
        Some("spawn_agent") => {
            if let Some(child) = string_arg(&args, "session_id") {
                CallKind::Message(MessageChildRequest { session_id: child, prompt })
            } else {
                CallKind::Spawn(SpawnChildRequest {
                    adapter: normalize_adapter(
                        args.get("adapter").and_then(Value::as_str).unwrap_or(""),
                    ),
                    prompt,
                    model: string_arg(&args, "model"),
                    agent_profile: string_arg(&args, "agent_profile"),
                    budget_usd: args.get("budget_usd").and_then(Value::as_f64),
                    cwd: string_arg(&args, "cwd"),
                    permission_mode: string_arg(&args, "permission_mode")
                        .and_then(|m| serde_json::from_value(Value::String(m)).ok()),
                    name: string_arg(&args, "name"),
                })
            }
        }
        _ => return Err("unsupported request kind".to_owned()),
    };
    Ok(Call { session_id, kind, timeout, proto })
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

fn reply_frame(outcome: &crate::childwatch::ChildOutcome) -> Value {
    let id_line = outcome
        .local_id
        .as_deref()
        .map(|id| format!("\n\n[child session id: {id} — pass it as session_id to follow up]"))
        .unwrap_or_default();
    match (&outcome.error, &outcome.final_text) {
        (Some(err), Some(text)) => json!({
            "ok": false,
            "error": format!("child agent failed: {err}\n\nlast output:\n{text}{id_line}"),
        }),
        (Some(err), None) => {
            json!({ "ok": false, "error": format!("child agent failed: {err}{id_line}") })
        }
        (None, Some(text)) => json!({ "ok": true, "result": format!("{text}{id_line}") }),
        (None, None) => json!({
            "ok": true,
            "result": format!("child agent finished without producing any output{id_line}"),
        }),
    }
}

/// Follow the child until it finishes or `timeout` elapses, streaming progress
/// frames to a proto≥2 relay via `out`.
async fn follow_child(
    handle: &WatchHandle,
    child_id: &str,
    timeout: Duration,
    proto: u64,
    out: &mut (impl AsyncWriteExt + Unpin),
) -> Value {
    follow_child_with(handle, child_id, timeout, SILENT_CHILD_GRACE, proto, out).await
}

/// Whether the child has produced any evidence of a running turn: a bound
/// session id alone only proves the harness registered it.
const fn showed_activity(snap: &crate::childwatch::ChildSnapshot) -> bool {
    snap.final_text.is_some()
        || snap.last_tool.is_some()
        || snap.status_line.is_some()
        || snap.blocked.is_some()
}

async fn follow_child_with(
    handle: &WatchHandle,
    child_id: &str,
    timeout: Duration,
    silent_grace: Duration,
    proto: u64,
    out: &mut (impl AsyncWriteExt + Unpin),
) -> Value {
    let started = Instant::now();
    let mut last_progress = Instant::now();
    loop {
        handle.changed(Duration::from_secs(2)).await;
        let now = Instant::now();
        let Some(snap) = handle.snapshot() else {
            return json!({ "ok": false, "error": "child agent tracking was dropped" });
        };
        match snap.assess(now) {
            Assessment::Finished(outcome) => return reply_frame(&outcome),
            Assessment::Running(line) => {
                if now.duration_since(started) >= silent_grace && !showed_activity(&snap) {
                    return json!({
                        "ok": false,
                        "error": format!(
                            "child agent {} produced no activity within {}s of being prompted — \
                             no model call, no output and no error, so it almost certainly died \
                             at startup (auth, budget or rate-limit rejection). Not waiting out \
                             the {}s timeout; check the child session in cctui.",
                            snap.local_id.as_deref().unwrap_or(child_id),
                            silent_grace.as_secs(),
                            timeout.as_secs(),
                        ),
                    });
                }
                if now.duration_since(started) >= timeout {
                    return json!({
                        "ok": false,
                        "error": format!(
                            "child agent {child_id} did not finish within {}s — it is still \
                             running; check it in cctui, or follow up by calling CctuiAgent \
                             with session_id {:?}",
                            timeout.as_secs(),
                            snap.local_id.as_deref().unwrap_or(child_id),
                        ),
                    });
                }
                if proto >= 2 && now.duration_since(last_progress) >= PROGRESS_EVERY {
                    last_progress = now;
                    let frame = json!({
                        "progress": format!(
                            "[{}s] {} · child session {}",
                            now.duration_since(started).as_secs(),
                            snippet(&line, 300),
                            snap.local_id.as_deref().unwrap_or(child_id),
                        ),
                    });
                    if write_line(out, &frame).await.is_err() {
                        return json!({ "ok": false, "error": "relay went away" });
                    }
                }
            }
        }
    }
}

async fn write_line(out: &mut (impl AsyncWriteExt + Unpin), frame: &Value) -> std::io::Result<()> {
    out.write_all(format!("{frame}\n").as_bytes()).await?;
    out.flush().await
}

async fn run_call(
    server: &ServerClient,
    machine_key: &str,
    call: Call,
    out: &mut (impl AsyncWriteExt + Unpin),
) -> Value {
    let watch = crate::childwatch::global();
    let (handle, child_id) = match &call.kind {
        CallKind::Spawn(req) => {
            let child = match server.spawn_child(machine_key, &call.session_id, req).await {
                Ok(child) => child,
                Err(err) => return json!({ "ok": false, "error": err.to_string() }),
            };
            // Register BEFORE the spawn frame can produce events; the server
            // has already dispatched the spawn at this point, but the child
            // takes seconds to boot, so this stays ahead of its first event.
            let handle = watch.register(&child.session_id);
            tracing::info!(
                parent = %call.session_id,
                child = %child.session_id,
                adapter = %req.adapter,
                "CctuiAgent following spawned child",
            );
            (handle, child.session_id)
        }
        CallKind::Message(req) => {
            let handle = watch.register_bound(&req.session_id);
            if let Err(err) = server.message_child(machine_key, &call.session_id, req).await {
                return json!({ "ok": false, "error": err.to_string() });
            }
            tracing::info!(
                parent = %call.session_id,
                child = %req.session_id,
                "CctuiAgent following child after follow-up",
            );
            (handle, req.session_id.clone())
        }
    };
    follow_child(&handle, &child_id, call.timeout, call.proto, out).await
}

async fn handle_connection(stream: UnixStream, server: ServerClient, machine_key: String) {
    let (read_half, mut write_half) = stream.into_split();
    let mut lines = BufReader::new(read_half).lines();
    let Ok(Some(line)) = lines.next_line().await else { return };
    let frame = match parse_call(&line) {
        Ok(call) => run_call(&server, &machine_key, call, &mut write_half).await,
        Err(err) => json!({ "ok": false, "error": err }),
    };
    let _ = write_line(&mut write_half, &frame).await;
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
    fn parses_a_full_spawn_call() {
        let line = json!({
            "kind": "spawn_agent",
            "session_id": "parent-1",
            "timeout_secs": 120,
            "proto": 2,
            "args": {
                "adapter": "opencode",
                "prompt": "review the diff",
                "model": "accounts/fireworks/models/kimi-k3",
                "agent_profile": "cctui-reviewer",
                "budget_usd": 0.5,
                "cwd": "/workspace",
                "permission_mode": "auto",
                "name": "reviewer",
            },
        })
        .to_string();
        let call = parse_call(&line).unwrap();
        assert_eq!(call.session_id, "parent-1");
        assert_eq!(call.timeout, Duration::from_mins(2));
        assert_eq!(call.proto, 2);
        let CallKind::Spawn(req) = call.kind else { panic!("expected spawn") };
        assert_eq!(req.adapter, "opencode");
        assert_eq!(req.agent_profile.as_deref(), Some("cctui-reviewer"));
        assert_eq!(req.budget_usd, Some(0.5));
        assert_eq!(req.cwd.as_deref(), Some("/workspace"));
        assert_eq!(req.permission_mode, Some(cctui_proto::adapter::PermissionMode::Auto));
        assert_eq!(req.name.as_deref(), Some("reviewer"));
    }

    #[test]
    fn a_session_id_arg_turns_the_call_into_a_follow_up() {
        let line = json!({
            "kind": "spawn_agent",
            "session_id": "parent-1",
            "args": { "session_id": "child-9", "prompt": "and check the tests" },
        })
        .to_string();
        let call = parse_call(&line).unwrap();
        let CallKind::Message(req) = call.kind else { panic!("expected message") };
        assert_eq!(req.session_id, "child-9");
        assert_eq!(req.prompt, "and check the tests");
    }

    #[test]
    fn proto_defaults_to_1_for_old_relays() {
        let line = json!({
            "kind": "spawn_agent",
            "session_id": "p",
            "args": { "adapter": "codex", "prompt": "go" },
        })
        .to_string();
        assert_eq!(parse_call(&line).unwrap().proto, 1);
    }

    #[test]
    fn blank_optional_args_are_dropped_not_forwarded_empty() {
        let line = json!({
            "kind": "spawn_agent",
            "session_id": "p",
            "args": { "adapter": "codex", "prompt": "go", "model": "  ", "cwd": "",
                      "permission_mode": "notamode" },
        })
        .to_string();
        let call = parse_call(&line).unwrap();
        let CallKind::Spawn(req) = call.kind else { panic!("expected spawn") };
        assert!(req.model.is_none());
        assert!(req.cwd.is_none());
        assert!(req.permission_mode.is_none());
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
        let out = |text: Option<&str>, err: Option<&str>| ChildOutcome {
            final_text: text.map(str::to_owned),
            error: err.map(str::to_owned),
            local_id: Some("child-7".into()),
        };
        let ok = reply_frame(&out(Some("verdict: ship"), None));
        assert_eq!(ok["ok"], json!(true));
        let text = ok["result"].as_str().unwrap();
        assert!(text.starts_with("verdict: ship"));
        assert!(text.contains("child-7"), "reply must carry the child id: {text}");

        let failed = reply_frame(&out(None, Some("crashed")));
        assert_eq!(failed["ok"], json!(false));
        assert!(failed["error"].as_str().unwrap().contains("crashed"));

        let partial = reply_frame(&out(Some("got halfway"), Some("killed")));
        assert_eq!(partial["ok"], json!(false));
        let text = partial["error"].as_str().unwrap();
        assert!(text.contains("killed") && text.contains("got halfway"));

        let silent = reply_frame(&out(None, None));
        assert_eq!(silent["ok"], json!(true));
        assert!(silent["result"].as_str().unwrap().contains("without producing any output"));
    }

    #[tokio::test]
    async fn follow_child_streams_progress_then_final_result() {
        let watch = std::sync::Arc::new(crate::childwatch::ChildWatch::default());
        let handle = watch.register("child-1");
        let observer = watch.clone();
        let feeder = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            observer.observe(&cctui_proto::adapter::AdapterEvent::Message {
                local_id: "child-1".into(),
                payload: json!({ "role": "assistant", "text": "all done" }),
            });
            observer.observe(&cctui_proto::adapter::AdapterEvent::SessionEnded {
                local_id: "child-1".into(),
                reason: cctui_proto::adapter::EndReason::Completed,
            });
        });
        let mut out: Vec<u8> = Vec::new();
        let frame = follow_child(&handle, "child-1", Duration::from_secs(10), 2, &mut out).await;
        feeder.await.unwrap();
        assert_eq!(frame["ok"], json!(true));
        assert!(frame["result"].as_str().unwrap().starts_with("all done"));
    }

    #[tokio::test]
    async fn follow_child_times_out_with_a_follow_up_hint() {
        let watch = std::sync::Arc::new(crate::childwatch::ChildWatch::default());
        let handle = watch.register("child-1");
        watch.observe(&cctui_proto::adapter::AdapterEvent::SessionStarted {
            local_id: "child-1".into(),
            meta: cctui_proto::adapter::SessionMeta::default(),
        });
        let mut out: Vec<u8> = Vec::new();
        let frame = follow_child(&handle, "child-1", Duration::from_millis(10), 1, &mut out).await;
        assert_eq!(frame["ok"], json!(false));
        let text = frame["error"].as_str().unwrap();
        assert!(text.contains("still running"), "{text}");
        assert!(text.contains("session_id"), "{text}");
        assert!(out.is_empty(), "proto 1 must never receive progress frames");
    }

    #[tokio::test]
    async fn a_silent_child_fails_fast_instead_of_waiting_out_the_timeout() {
        let watch = std::sync::Arc::new(crate::childwatch::ChildWatch::default());
        let handle = watch.register("child-1");
        watch.observe(&cctui_proto::adapter::AdapterEvent::SessionStarted {
            local_id: "child-1".into(),
            meta: cctui_proto::adapter::SessionMeta::default(),
        });
        let mut out: Vec<u8> = Vec::new();
        let frame = follow_child_with(
            &handle,
            "child-1",
            Duration::from_mins(30),
            Duration::from_millis(10),
            2,
            &mut out,
        )
        .await;
        assert_eq!(frame["ok"], json!(false));
        let text = frame["error"].as_str().unwrap();
        assert!(text.contains("no activity"), "{text}");
        assert!(text.contains("died at startup"), "{text}");
    }

    #[tokio::test]
    async fn a_child_that_showed_activity_is_never_failed_fast() {
        let watch = std::sync::Arc::new(crate::childwatch::ChildWatch::default());
        let handle = watch.register("child-1");
        watch.observe(&cctui_proto::adapter::AdapterEvent::SessionStarted {
            local_id: "child-1".into(),
            meta: cctui_proto::adapter::SessionMeta::default(),
        });
        watch.observe(&cctui_proto::adapter::AdapterEvent::ToolUse {
            local_id: "child-1".into(),
            payload: json!({ "tool": "Bash" }),
        });
        let mut out: Vec<u8> = Vec::new();
        let frame = follow_child_with(
            &handle,
            "child-1",
            Duration::from_millis(20),
            Duration::from_millis(10),
            1,
            &mut out,
        )
        .await;
        let text = frame["error"].as_str().unwrap();
        assert!(
            text.contains("still running"),
            "a working child must hit the timeout path: {text}"
        );
    }

    #[tokio::test]
    async fn a_crashed_child_returns_before_the_timeout() {
        let watch = std::sync::Arc::new(crate::childwatch::ChildWatch::default());
        let handle = watch.register("child-1");
        let observer = watch.clone();
        let feeder = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            observer.observe(&cctui_proto::adapter::AdapterEvent::SessionEnded {
                local_id: "child-1".into(),
                reason: cctui_proto::adapter::EndReason::Crashed {
                    detail: "gateway rejected the first model call".into(),
                },
            });
        });
        let mut out: Vec<u8> = Vec::new();
        let started = Instant::now();
        let frame = follow_child_with(
            &handle,
            "child-1",
            Duration::from_mins(30),
            Duration::from_mins(30),
            2,
            &mut out,
        )
        .await;
        feeder.await.unwrap();
        assert!(started.elapsed() < Duration::from_secs(30), "must not wait out the timeout");
        assert_eq!(frame["ok"], json!(false));
        assert!(frame["error"].as_str().unwrap().contains("gateway rejected"));
    }

    #[test]
    fn tool_is_unavailable_without_a_machine_key() {
        assert!(!is_available(""));
        assert!(!is_available("   "));
        assert!(is_available("machine-key"));
    }
}
