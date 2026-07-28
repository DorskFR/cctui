//! `cctui-daemon mcp-agent` — the stdio MCP server a claude session is launched
//! with, exposing the single `CctuiAgent` tool.
//!
//! The subcommand is a thin relay, mirroring `ask-hook`: it speaks MCP on
//! stdio and forwards each `tools/call` to the long-lived daemon over its local
//! Unix socket, which owns the machine key and the spawn path. The session id is
//! fixed by the `--session` argv the daemon wrote into the session's MCP config,
//! so a session can never ask on another session's behalf.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};

pub const TOOL_NAME: &str = "CctuiAgent";

/// MCP protocol revision this server implements.
const PROTOCOL_VERSION: &str = "2024-11-05";

/// Ceiling on a single tool call, and the default when the call names none.
/// Generous: a child review session can legitimately run for many minutes.
const DEFAULT_TIMEOUT_SECS: u64 = 1800;
const MAX_TIMEOUT_SECS: u64 = 7200;

/// The `CctuiAgent` input schema, as advertised to the model.
#[must_use]
pub fn tool_schema() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": "Spawn a cctui subagent session and wait for it to finish. \
    The child is a real cctui session: it appears nested under this one in the UI, \
    its token usage is metered, its spend is capped, and it can be killed. \
    Returns the child's final message.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "adapter": {
                    "type": "string",
                    "description": "Harness to run the child under, e.g. \"opencode\", \
    \"codex\", \"claude_code\". Only the adapters this session is permitted to spawn are accepted.",
                },
                "prompt": {
                    "type": "string",
                    "description": "The task for the child agent.",
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
    (default 1800, max 7200). The child is killed on timeout.",
                },
            },
            "required": ["adapter", "prompt"],
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

/// One JSON-RPC response line for `id`.
fn reply(id: Option<&Value>, result: &Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id.cloned().unwrap_or(Value::Null), "result": result })
}

/// A tool result carrying `text`. `is_error` marks a failed call so the model
/// sees the failure instead of a hang.
fn tool_result(id: Option<&Value>, text: &str, is_error: bool) -> Value {
    reply(id, &json!({ "content": [{ "type": "text", "text": text }], "isError": is_error }))
}

/// Handle one decoded JSON-RPC request; `None` for notifications (no reply).
fn handle_request(session_id: &str, sock: &Path, req: &Value) -> Option<Value> {
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
            let (text, is_error) = call_daemon(session_id, sock, &args);
            Some(tool_result(id, &text, is_error))
        }
        // Notifications carry no id and expect no reply.
        _ if id.is_none() => None,
        _ => Some(reply(id, &json!({}))),
    }
}

/// Send the tool call to the daemon and block on its single-line reply.
/// Every failure returns text: the model must see an error, never a hang.
fn call_daemon(session_id: &str, sock: &Path, args: &Value) -> (String, bool) {
    let timeout = resolve_timeout(args.get("timeout_secs").and_then(Value::as_u64));
    let request = json!({
        "kind": "spawn_agent",
        "session_id": session_id,
        "args": args,
        "timeout_secs": timeout.as_secs(),
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
    let mut line = String::new();
    if BufReader::new(&stream).read_line(&mut line).is_err() || line.trim().is_empty() {
        return (
            "CctuiAgent failed: the daemon closed the connection without a result".to_owned(),
            true,
        );
    }
    let Ok(resp) = serde_json::from_str::<Value>(&line) else {
        return ("CctuiAgent failed: malformed daemon reply".to_owned(), true);
    };
    let ok = resp.get("ok").and_then(Value::as_bool).unwrap_or(false);
    let text = resp
        .get(if ok { "result" } else { "error" })
        .and_then(Value::as_str)
        .unwrap_or("no output")
        .to_owned();
    (text, !ok)
}

/// Serve MCP on stdio until the client closes it.
pub fn run(session_id: &str, sock: &Path) -> anyhow::Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(req) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(resp) = handle_request(session_id, sock, &req) {
            writeln!(stdout, "{resp}")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_schema_names_the_tool_and_its_required_args() {
        let schema = tool_schema();
        assert_eq!(schema["name"], TOOL_NAME);
        assert_eq!(schema["inputSchema"]["required"], json!(["adapter", "prompt"]));
        let props = schema["inputSchema"]["properties"].as_object().unwrap();
        for key in
            ["adapter", "prompt", "model", "agent_profile", "budget_usd", "cwd", "timeout_secs"]
        {
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
        let resp = handle_request("s1", Path::new("/tmp/x.sock"), &req).unwrap();
        assert_eq!(resp["id"], json!(1));
        assert_eq!(resp["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert!(resp["result"]["capabilities"]["tools"].is_object());
    }

    #[test]
    fn tools_list_returns_exactly_the_agent_tool() {
        let req = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" });
        let resp = handle_request("s1", Path::new("/tmp/x.sock"), &req).unwrap();
        let tools = resp["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], TOOL_NAME);
    }

    #[test]
    fn notifications_get_no_reply() {
        let req = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        assert!(handle_request("s1", Path::new("/tmp/x.sock"), &req).is_none());
    }

    #[test]
    fn unknown_tool_is_an_error_result_not_a_hang() {
        let req = json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": { "name": "SomethingElse", "arguments": {} },
        });
        let resp = handle_request("s1", Path::new("/tmp/x.sock"), &req).unwrap();
        assert_eq!(resp["result"]["isError"], json!(true));
    }

    #[test]
    fn a_dead_daemon_socket_returns_an_error_result() {
        let req = json!({
            "jsonrpc": "2.0", "id": 4, "method": "tools/call",
            "params": {
                "name": TOOL_NAME,
                "arguments": { "adapter": "opencode", "prompt": "hi", "timeout_secs": 1 },
            },
        });
        let resp = handle_request("s1", Path::new("/nonexistent/cctui-agent.sock"), &req).unwrap();
        assert_eq!(resp["result"]["isError"], json!(true));
        assert!(
            resp["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("cannot reach the cctui daemon")
        );
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
