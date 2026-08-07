//! `cctui-daemon mcp-agent` — the stdio MCP server a claude session is launched
//! with, exposing the single `CctuiAgent` tool.
//!
//! The subcommand is a thin relay, mirroring `ask-hook`: it speaks MCP on
//! stdio and forwards each `tools/call` to the long-lived daemon over its local
//! Unix socket, which owns the machine key and the spawn path. The session id is
//! fixed by the `--session` argv the daemon wrote into the session's MCP config,
//! so a session can never ask on another session's behalf.
//!
//! Calls run concurrently — one thread per `tools/call`, replies keyed by
//! JSON-RPC id — so parallel child spawns actually run in parallel. While a
//! call waits, the daemon's interim progress frames are forwarded as MCP
//! `notifications/progress` (when the client sent a `progressToken`), which
//! resets the client's tool idle timeout: a long-running child no longer
//! looks like a dead call.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

pub const TOOL_NAME: &str = "CctuiAgent";

/// MCP protocol revision this server implements.
const PROTOCOL_VERSION: &str = "2024-11-05";

/// Socket line protocol revision: ≥2 tells the daemon this relay understands
/// interim `progress` frames before the final result line.
const SOCKET_PROTO: u64 = 2;

/// Ceiling on a single tool call, and the default when the call names none.
/// Generous: a child review session can legitimately run for many minutes.
const DEFAULT_TIMEOUT_SECS: u64 = 1800;
const MAX_TIMEOUT_SECS: u64 = 7200;

/// The `CctuiAgent` input schema, as advertised to the model.
#[must_use]
pub fn tool_schema() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": "Spawn a cctui subagent session, follow it while it works, and return \
    its final message. The child is a real cctui session: it appears nested under this one in \
    the UI, its token usage is metered, its spend is capped, and it can be killed. Progress \
    (current tool, status, latest message) streams back while it runs. Parallel calls are \
    supported. To send a follow-up prompt to a child from an earlier call, pass its session_id \
    (returned in the reply) together with the new prompt.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "adapter": {
                    "type": "string",
                    "description": "Harness to run the child under, e.g. \"opencode\", \
    \"codex\", \"claude_code\". Only the adapters this session is permitted to spawn are \
    accepted. Ignored when session_id is set.",
                },
                "prompt": {
                    "type": "string",
                    "description": "The task for the child agent (or the follow-up message \
    when session_id is set).",
                },
                "session_id": {
                    "type": "string",
                    "description": "Session id of a child spawned earlier by this session: \
    send `prompt` to it as a follow-up and wait for its answer instead of spawning anew.",
                },
                "model": {
                    "type": "string",
                    "description": "Model id from the account's catalog. Omit for the account default.",
                },
                "agent_profile": {
                    "type": "string",
                    "description": "Named agent profile to run under, e.g. \"cctui-reviewer\" \
    (a locked-down opencode reviewer).",
                },
                "permission_mode": {
                    "type": "string",
                    "enum": ["yolo", "auto", "ask"],
                    "description": "Child permission posture. Default \"yolo\" (no prompts — \
    like a Task subagent, nobody is attached to answer them).",
                },
                "name": {
                    "type": "string",
                    "description": "Display name for the child session in the UI.",
                },
                "budget_usd": {
                    "type": "number",
                    "description": "Dollar ceiling for this child's own spend. Must not exceed \
    this session's permitted maximum; omit to inherit it.",
                },
                "cwd": {
                    "type": "string",
                    "description": "Working directory for the child. Defaults to this session's.",
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "How long to wait for the child before giving up \
    (default 1800, max 7200). On timeout the child keeps running and can be followed up \
    via session_id.",
                },
            },
            "required": ["prompt"],
            "additionalProperties": false,
        },
    })
}

/// Clamp a caller-supplied timeout into the supported range.
#[must_use]
pub fn resolve_timeout(requested: Option<u64>) -> Duration {
    Duration::from_secs(requested.unwrap_or(DEFAULT_TIMEOUT_SECS).clamp(1, MAX_TIMEOUT_SECS))
}

/// Build the `.mcp.json`-shaped config registering this server for a session.
///
/// `exe` is the daemon binary; the session id and socket are baked into argv so
/// the tool call carries no session identity of its own.
#[must_use]
pub fn mcp_config(exe: &str, session_id: &str, sock: &Path) -> Value {
    json!({
        "mcpServers": {
            "cctui": {
                "type": "stdio",
                "command": exe,
                "args": [
                    "mcp-agent",
                    "--session", session_id,
                    "--sock", sock.to_string_lossy(),
                ],
            }
        }
    })
}

/// Serialize one JSON-RPC frame to stdout. The lock is per-line so concurrent
/// tool calls interleave whole frames, never bytes.
#[derive(Clone)]
struct Outbox(Arc<Mutex<std::io::Stdout>>);

impl Outbox {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(std::io::stdout())))
    }

    fn send(&self, frame: &Value) {
        if let Ok(mut out) = self.0.lock() {
            let _ = writeln!(out, "{frame}");
            let _ = out.flush();
        }
    }
}

fn reply(id: Option<&Value>, result: &Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id.cloned().unwrap_or(Value::Null), "result": result })
}

/// A tool result carrying `text`. `is_error` marks a failed call so the model
/// sees the failure instead of a hang.
fn tool_result(id: Option<&Value>, text: &str, is_error: bool) -> Value {
    reply(id, &json!({ "content": [{ "type": "text", "text": text }], "isError": is_error }))
}

fn progress_notification(token: &Value, seq: u64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "notifications/progress",
        "params": { "progressToken": token, "progress": seq, "message": message },
    })
}

/// The `_meta.progressToken` of a request, when the client sent one.
fn progress_token(req: &Value) -> Option<Value> {
    req.pointer("/params/_meta/progressToken").cloned().filter(|t| !t.is_null())
}

/// Handle one decoded JSON-RPC request inline; `None` for notifications (no
/// reply) AND for `tools/call`, which replies asynchronously from its own
/// thread via the outbox.
fn handle_request(session_id: &str, sock: &Path, req: &Value, outbox: &Outbox) -> Option<Value> {
    let method = req.get("method").and_then(Value::as_str)?;
    let id = req.get("id");
    match method {
        "initialize" => Some(reply(
            id,
            &json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "cctui", "version": env!("CARGO_PKG_VERSION") },
            }),
        )),
        "tools/list" => Some(reply(id, &json!({ "tools": [tool_schema()] }))),
        "tools/call" => {
            let params = req.get("params");
            let name = params.and_then(|p| p.get("name")).and_then(Value::as_str).unwrap_or("");
            if name != TOOL_NAME {
                return Some(tool_result(id, &format!("unknown tool {name:?}"), true));
            }
            let args =
                params.and_then(|p| p.get("arguments")).cloned().unwrap_or_else(|| json!({}));
            let id = id.cloned();
            let token = progress_token(req);
            let session_id = session_id.to_owned();
            let sock = sock.to_owned();
            let outbox = outbox.clone();
            std::thread::spawn(move || {
                let (text, is_error) =
                    call_daemon(&session_id, &sock, &args, token.as_ref(), &outbox);
                outbox.send(&tool_result(id.as_ref(), &text, is_error));
            });
            None
        }
        _ if id.is_none() => None,
        _ => Some(reply(id, &json!({}))),
    }
}

/// Send the tool call to the daemon, forwarding interim progress frames, and
/// block on the final result line. Every failure returns text: the model must
/// see an error, never a hang.
fn call_daemon(
    session_id: &str,
    sock: &Path,
    args: &Value,
    token: Option<&Value>,
    outbox: &Outbox,
) -> (String, bool) {
    let timeout = resolve_timeout(args.get("timeout_secs").and_then(Value::as_u64));
    let request = json!({
        "kind": "spawn_agent",
        "session_id": session_id,
        "args": args,
        "timeout_secs": timeout.as_secs(),
        "proto": SOCKET_PROTO,
    });
    let stream = match UnixStream::connect(sock) {
        Ok(s) => s,
        Err(err) => {
            return (
                format!("CctuiAgent unavailable: cannot reach the cctui daemon ({err})"),
                true,
            );
        }
    };
    // Outlive the daemon's own wait so the daemon's timeout message wins.
    let _ = stream.set_read_timeout(Some(timeout + Duration::from_secs(30)));
    let mut writer = &stream;
    if writeln!(writer, "{request}").and_then(|()| writer.flush()).is_err() {
        return ("CctuiAgent failed: could not send the request to the daemon".to_owned(), true);
    }
    let mut reader = BufReader::new(&stream);
    let mut seq: u64 = 0;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
            return (
                "CctuiAgent failed: the daemon closed the connection without a result".to_owned(),
                true,
            );
        }
        let Ok(frame) = serde_json::from_str::<Value>(&line) else {
            return ("CctuiAgent failed: malformed daemon reply".to_owned(), true);
        };
        if let Some(progress) = frame.get("progress").and_then(Value::as_str) {
            if let Some(token) = token {
                seq += 1;
                outbox.send(&progress_notification(token, seq, progress));
            }
            continue;
        }
        let ok = frame.get("ok").and_then(Value::as_bool).unwrap_or(false);
        let text = frame
            .get(if ok { "result" } else { "error" })
            .and_then(Value::as_str)
            .unwrap_or("no output")
            .to_owned();
        return (text, !ok);
    }
}

/// Serve MCP on stdio until the client closes it.
pub fn run(session_id: &str, sock: &Path) -> anyhow::Result<()> {
    let stdin = std::io::stdin();
    let outbox = Outbox::new();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(req) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(resp) = handle_request(session_id, sock, &req, &outbox) {
            outbox.send(&resp);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handle(session_id: &str, sock: &Path, req: &Value) -> Option<Value> {
        handle_request(session_id, sock, req, &Outbox::new())
    }

    #[test]
    fn tool_schema_names_the_tool_and_its_required_args() {
        let schema = tool_schema();
        assert_eq!(schema["name"], TOOL_NAME);
        assert_eq!(schema["inputSchema"]["required"], json!(["prompt"]));
        let props = schema["inputSchema"]["properties"].as_object().unwrap();
        for key in [
            "adapter",
            "prompt",
            "session_id",
            "model",
            "agent_profile",
            "permission_mode",
            "name",
            "budget_usd",
            "cwd",
            "timeout_secs",
        ] {
            assert!(props.contains_key(key), "{key} missing from the schema");
        }
        assert_eq!(props["budget_usd"]["type"], "number");
        assert_eq!(schema["inputSchema"]["additionalProperties"], json!(false));
    }

    #[test]
    fn tool_schema_round_trips_as_json() {
        let raw = serde_json::to_string(&tool_schema()).unwrap();
        let back: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(back, tool_schema());
    }

    #[test]
    fn initialize_advertises_tools() {
        let req = json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize" });
        let resp = handle("s1", Path::new("/tmp/x.sock"), &req).unwrap();
        assert_eq!(resp["id"], json!(1));
        assert_eq!(resp["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert!(resp["result"]["capabilities"]["tools"].is_object());
    }

    #[test]
    fn tools_list_returns_exactly_the_agent_tool() {
        let req = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" });
        let resp = handle("s1", Path::new("/tmp/x.sock"), &req).unwrap();
        let tools = resp["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], TOOL_NAME);
    }

    #[test]
    fn notifications_get_no_reply() {
        let req = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        assert!(handle("s1", Path::new("/tmp/x.sock"), &req).is_none());
    }

    #[test]
    fn unknown_tool_is_an_error_result_not_a_hang() {
        let req = json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": { "name": "SomethingElse", "arguments": {} },
        });
        let resp = handle("s1", Path::new("/tmp/x.sock"), &req).unwrap();
        assert_eq!(resp["result"]["isError"], json!(true));
    }

    #[test]
    fn a_dead_daemon_socket_returns_an_error_result() {
        let (text, is_error) = call_daemon(
            "s1",
            Path::new("/nonexistent/cctui-agent.sock"),
            &json!({ "adapter": "opencode", "prompt": "hi", "timeout_secs": 1 }),
            None,
            &Outbox::new(),
        );
        assert!(is_error);
        assert!(text.contains("cannot reach the cctui daemon"));
    }

    #[test]
    fn progress_frames_forward_as_notifications_and_final_line_ends_the_call() {
        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("agent.sock");
        let listener = std::os::unix::net::UnixListener::bind(&sock_path).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap()).read_line(&mut line).unwrap();
            let req: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(req["proto"], json!(SOCKET_PROTO));
            writeln!(stream, "{}", json!({ "progress": "child working · tool: Bash" })).unwrap();
            writeln!(stream, "{}", json!({ "ok": true, "result": "verdict: ship" })).unwrap();
        });
        let (text, is_error) = call_daemon(
            "s1",
            &sock_path,
            &json!({ "adapter": "codex", "prompt": "go", "timeout_secs": 5 }),
            Some(&json!("tok-1")),
            &Outbox::new(),
        );
        server.join().unwrap();
        assert!(!is_error);
        assert_eq!(text, "verdict: ship");
    }

    #[test]
    fn concurrent_tool_calls_reply_out_of_order() {
        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("agent.sock");
        let listener = std::os::unix::net::UnixListener::bind(&sock_path).unwrap();
        let server = std::thread::spawn(move || {
            let mut streams = Vec::new();
            for _ in 0..2 {
                let (stream, _) = listener.accept().unwrap();
                let mut line = String::new();
                BufReader::new(stream.try_clone().unwrap()).read_line(&mut line).unwrap();
                let req: Value = serde_json::from_str(&line).unwrap();
                streams.push((stream, req["args"]["prompt"].as_str().unwrap().to_owned()));
            }
            // Answer in reverse arrival order: the second call must not be
            // blocked behind the first.
            streams.reverse();
            for (mut stream, prompt) in streams {
                writeln!(stream, "{}", json!({ "ok": true, "result": format!("done: {prompt}") }))
                    .unwrap();
            }
        });
        let sock_a = sock_path.clone();
        let a = std::thread::spawn(move || {
            call_daemon(
                "s1",
                &sock_a,
                &json!({ "adapter": "codex", "prompt": "first", "timeout_secs": 5 }),
                None,
                &Outbox::new(),
            )
        });
        std::thread::sleep(Duration::from_millis(50));
        let b = std::thread::spawn(move || {
            call_daemon(
                "s1",
                &sock_path,
                &json!({ "adapter": "codex", "prompt": "second", "timeout_secs": 5 }),
                None,
                &Outbox::new(),
            )
        });
        let (text_a, err_a) = a.join().unwrap();
        let (text_b, err_b) = b.join().unwrap();
        server.join().unwrap();
        assert!(!err_a && !err_b);
        assert_eq!(text_a, "done: first");
        assert_eq!(text_b, "done: second");
    }

    #[test]
    fn progress_token_is_read_from_meta() {
        let req = json!({
            "jsonrpc": "2.0", "id": 4, "method": "tools/call",
            "params": {
                "name": TOOL_NAME,
                "_meta": { "progressToken": 7 },
                "arguments": {},
            },
        });
        assert_eq!(progress_token(&req), Some(json!(7)));
        assert!(progress_token(&json!({ "params": {} })).is_none());
    }

    #[test]
    fn timeout_is_clamped_to_the_supported_range() {
        assert_eq!(resolve_timeout(None).as_secs(), DEFAULT_TIMEOUT_SECS);
        assert_eq!(resolve_timeout(Some(60)).as_secs(), 60);
        assert_eq!(resolve_timeout(Some(0)).as_secs(), 1);
        assert_eq!(resolve_timeout(Some(999_999)).as_secs(), MAX_TIMEOUT_SECS);
    }

    #[test]
    fn mcp_config_bakes_the_session_and_socket_into_argv() {
        let cfg = mcp_config("/usr/bin/cctui-daemon", "sess-1", Path::new("/run/cctui/agent.sock"));
        let server = &cfg["mcpServers"]["cctui"];
        assert_eq!(server["command"], "/usr/bin/cctui-daemon");
        assert_eq!(server["type"], "stdio");
        assert_eq!(
            server["args"],
            json!(["mcp-agent", "--session", "sess-1", "--sock", "/run/cctui/agent.sock"])
        );
    }
}
