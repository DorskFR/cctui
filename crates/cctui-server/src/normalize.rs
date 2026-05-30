//! Translate adapter-native `stream_events` payloads into the canonical
//! cctui client shape. Stream events are stored as the adapter emitted
//! them (raw, labelled by `sessions.adapter_id`); the normalisation
//! happens on read so clients can keep one renderer.
//!
//! Canonical client shapes:
//! - `{ "type": "text",        "content", "ts"?, "role"? }`
//! - `{ "type": "tool_call",   "tool", "input", "ts"? }`
//! - `{ "type": "tool_result", "output_summary", "ts"? }`
//! - `{ "type": "reply",       "content", "ts"? }`
//!
//! Returning `None` drops the row from the conversation view (e.g.
//! `session_ended` markers, summaries with no UI value).

use cctui_proto::ws::AgentEvent;
use serde_json::{Value, json};

/// Build the canonical [`AgentEvent`] shape for a fresh daemon-side
/// payload — used to fan out live updates over the TUI broadcast so the
/// drawer renders them as they arrive (without waiting for the next
/// `get_conversation` poll). `adapter_id` selects the adapter's payload
/// dialect (codex emits its own item shapes, not the claude ones).
#[must_use]
pub fn to_agent_event(adapter_id: &str, event_type: &str, payload: &Value) -> Option<AgentEvent> {
    let ts = chrono::Utc::now().timestamp_millis();
    if adapter_id == "codex" {
        // Reuse the read-side canonicalizer, then lift the canonical client
        // Value into the live `AgentEvent` shape — one source of truth for
        // codex's payload dialect.
        return codex(event_type, payload).and_then(|v| agent_event_from_canonical(&v, ts));
    }
    match event_type {
        "message" => {
            let role = payload.get("role").and_then(Value::as_str)?;
            let text = payload.get("text").and_then(Value::as_str).unwrap_or_default();
            match role {
                "user" => Some(AgentEvent::Text { content: format!("▷ User: {text}"), ts }),
                "assistant" | "assistant_thinking" => {
                    Some(AgentEvent::Text { content: text.to_owned(), ts })
                }
                _ => None,
            }
        }
        "tool_use" => {
            if payload.get("kind").and_then(Value::as_str) == Some("tool_result") {
                let summary = payload
                    .get("content")
                    .and_then(|v| v.as_str().map(str::to_owned).or_else(|| Some(v.to_string())))
                    .unwrap_or_default();
                return Some(AgentEvent::ToolResult {
                    tool: String::new(),
                    output_summary: summary,
                    ts,
                });
            }
            let tool = payload.get("tool")?.as_str()?.to_owned();
            let input = payload.get("input").cloned().unwrap_or(Value::Null);
            Some(AgentEvent::ToolCall { tool, input, ts })
        }
        _ => None,
    }
}

#[must_use]
pub fn for_client(adapter_id: &str, event_type: &str, payload: Value) -> Option<Value> {
    match adapter_id {
        "claude-code" => claude_code(event_type, payload),
        "codex" => codex(event_type, &payload),
        // Future adapters: add their mappers here.
        _ => passthrough_if_canonical(payload),
    }
}

/// Lift a canonical client Value (`{type:"text"|"tool_call"|"tool_result", …}`)
/// into the live [`AgentEvent`] broadcast shape. Used so the codex live path
/// shares one mapper with the read path.
fn agent_event_from_canonical(v: &Value, ts: i64) -> Option<AgentEvent> {
    match v.get("type").and_then(Value::as_str)? {
        "text" => Some(AgentEvent::Text {
            content: v.get("content").and_then(Value::as_str).unwrap_or_default().to_owned(),
            ts,
        }),
        "tool_call" => Some(AgentEvent::ToolCall {
            tool: v.get("tool").and_then(Value::as_str).unwrap_or_default().to_owned(),
            input: v.get("input").cloned().unwrap_or(Value::Null),
            ts,
        }),
        "tool_result" => Some(AgentEvent::ToolResult {
            tool: String::new(),
            output_summary: v
                .get("output_summary")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            ts,
        }),
        _ => None,
    }
}

/// Map a codex app-server / log-tail event payload onto the canonical client
/// shape. Codex stores the raw `item/completed` item (CCT-137); its `type`
/// field is a codex-native `ThreadItem` discriminant (verified against
/// `codex app-server generate-json-schema`, codex-cli 0.135). The `event_type`
/// column (`message` / `tool_use`) is advisory only — we key off the item
/// `type`. Unknown items return `None` (dropped from the conversation).
fn codex(_event_type: &str, payload: &Value) -> Option<Value> {
    match payload.get("type").and_then(Value::as_str).unwrap_or_default() {
        // Final assistant answer + plan updates → assistant text.
        "agentMessage" | "plan" => {
            let text = payload.get("text").and_then(Value::as_str).unwrap_or_default();
            if text.is_empty() {
                return None;
            }
            Some(json!({ "type": "text", "content": text, "role": "Assistant" }))
        }
        // Model reasoning → text (content is an array of {type, text}).
        "reasoning" => {
            let text = codex_content_text(payload.get("content"));
            if text.is_empty() {
                return None;
            }
            Some(json!({ "type": "text", "content": text, "role": "Reasoning" }))
        }
        // User turn input (content is an array of {type:"text", text}).
        "userMessage" => {
            let text = codex_content_text(payload.get("content"));
            if text.is_empty() {
                return None;
            }
            Some(json!({ "type": "text", "content": format!("▷ User: {text}") }))
        }
        // Shell command: fold command + result into one tool_call so the
        // command *and* its output are visible (the renderer dumps unknown
        // tools' input as JSON).
        "commandExecution" => {
            let command = payload.get("command").and_then(Value::as_str).unwrap_or_default();
            let mut input = json!({ "command": command });
            if let Some(cwd) = payload.get("cwd") {
                input["cwd"] = cwd.clone();
            }
            if let Some(code) = payload.get("exitCode") {
                input["exit_code"] = code.clone();
            }
            if let Some(out) = payload.get("aggregatedOutput").and_then(Value::as_str) {
                // Cap output so a noisy command can't blow up the payload.
                let capped: String = out.chars().take(4000).collect();
                input["output"] = json!(capped);
            }
            Some(json!({ "type": "tool_call", "tool": "shell", "input": input }))
        }
        "fileChange" => Some(json!({
            "type": "tool_call",
            "tool": "apply_patch",
            "input": { "changes": payload.get("changes").cloned().unwrap_or(Value::Null) },
        })),
        "mcpToolCall" => {
            let server = payload.get("server").and_then(Value::as_str).unwrap_or_default();
            let tool = payload.get("tool").and_then(Value::as_str).unwrap_or_default();
            Some(json!({
                "type": "tool_call",
                "tool": format!("mcp__{server}__{tool}"),
                "input": payload.get("arguments").cloned().unwrap_or(Value::Null),
            }))
        }
        "webSearch" => Some(json!({
            "type": "tool_call",
            "tool": "WebSearch",
            "input": { "query": payload.get("query").and_then(Value::as_str).unwrap_or_default() },
        })),
        _ => None,
    }
}

/// Join the `text` fields of a codex content array (`userMessage.content`,
/// `reasoning.content`) into a single string.
fn codex_content_text(content: Option<&Value>) -> String {
    let Some(arr) = content.and_then(Value::as_array) else { return String::new() };
    arr.iter().filter_map(|c| c.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n")
}

fn claude_code(event_type: &str, payload: Value) -> Option<Value> {
    // Pre-normalized payloads already carry `type` — pass them through.
    if payload.get("type").and_then(Value::as_str).is_some() {
        return Some(payload);
    }
    match event_type {
        "message" => map_daemon_message(&payload),
        "tool_use" => map_daemon_tool(&payload),
        _ => None,
    }
}

fn map_daemon_message(payload: &Value) -> Option<Value> {
    let role = payload.get("role").and_then(Value::as_str)?;
    let text = payload.get("text").and_then(Value::as_str).unwrap_or_default();
    match role {
        "user" => Some(json!({ "type": "text", "content": format!("▷ User: {text}") })),
        "assistant" | "assistant_thinking" => Some(json!({
            "type": "text",
            "content": text,
            "role": "Assistant",
        })),
        "summary" => {
            // Post-turn summaries carry status_category / status_detail; surface
            // them only when there's something useful to display.
            let detail = payload.get("status_detail").and_then(Value::as_str).unwrap_or_default();
            if detail.is_empty() {
                None
            } else {
                Some(json!({ "type": "text", "content": format!("· {detail}") }))
            }
        }
        _ => None,
    }
}

fn map_daemon_tool(payload: &Value) -> Option<Value> {
    // ToolUse comes in two shapes: an assistant tool_use (id, tool, input)
    // and a user-side tool_result (kind="tool_result", content, ...).
    if payload.get("kind").and_then(Value::as_str) == Some("tool_result") {
        let summary = payload
            .get("content")
            .and_then(|v| v.as_str().map(str::to_owned).or_else(|| Some(v.to_string())))
            .unwrap_or_default();
        return Some(json!({ "type": "tool_result", "output_summary": summary }));
    }
    let tool = payload.get("tool")?.clone();
    let input = payload.get("input").cloned().unwrap_or(Value::Null);
    Some(json!({ "type": "tool_call", "tool": tool, "input": input }))
}

fn passthrough_if_canonical(payload: Value) -> Option<Value> {
    if payload.get("type").and_then(Value::as_str).is_some() { Some(payload) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_payload_passes_through() {
        let p = json!({ "type": "text", "content": "hi" });
        assert_eq!(for_client("claude-code", "message", p.clone()), Some(p));
    }

    #[test]
    fn daemon_assistant_text_maps_to_text() {
        let p = json!({ "role": "assistant", "text": "hello" });
        let n = for_client("claude-code", "message", p).unwrap();
        assert_eq!(n["type"], "text");
        assert_eq!(n["content"], "hello");
    }

    #[test]
    fn daemon_user_text_prefixed() {
        let p = json!({ "role": "user", "text": "hi" });
        let n = for_client("claude-code", "message", p).unwrap();
        assert_eq!(n["content"], "▷ User: hi");
    }

    #[test]
    fn daemon_tool_use_maps_to_tool_call() {
        let p = json!({ "id": "x", "tool": "Bash", "input": { "command": "ls" } });
        let n = for_client("claude-code", "tool_use", p).unwrap();
        assert_eq!(n["type"], "tool_call");
        assert_eq!(n["tool"], "Bash");
        assert_eq!(n["input"]["command"], "ls");
    }

    #[test]
    fn daemon_tool_result_maps_to_tool_result() {
        let p = json!({ "kind": "tool_result", "content": "ok", "is_error": false });
        let n = for_client("claude-code", "tool_use", p).unwrap();
        assert_eq!(n["type"], "tool_result");
        assert_eq!(n["output_summary"], "ok");
    }

    #[test]
    fn session_ended_dropped() {
        let p = json!({ "reason": { "Completed": {} } });
        assert_eq!(for_client("claude-code", "session_ended", p), None);
    }

    #[test]
    fn empty_summary_dropped() {
        let p = json!({ "role": "summary" });
        assert_eq!(for_client("claude-code", "message", p), None);
    }

    // --- codex normalizer (CCT-137); shapes captured from codex-cli 0.135 ---

    #[test]
    fn codex_agent_message_maps_to_text() {
        let p = json!({ "type": "agentMessage", "text": "`516d04a`", "phase": "final_answer" });
        let n = for_client("codex", "message", p).unwrap();
        assert_eq!(n["type"], "text");
        assert_eq!(n["content"], "`516d04a`");
        assert_eq!(n["role"], "Assistant");
    }

    #[test]
    fn codex_user_message_joins_content_and_prefixes() {
        let p = json!({ "type": "userMessage", "content": [
            { "type": "text", "text": "do a thing", "text_elements": [] }
        ]});
        let n = for_client("codex", "message", p).unwrap();
        assert_eq!(n["content"], "▷ User: do a thing");
    }

    #[test]
    fn codex_command_execution_maps_to_shell_tool_call() {
        let p = json!({
            "type": "commandExecution",
            "command": "git rev-parse --short HEAD",
            "cwd": "/repo",
            "aggregatedOutput": "516d04a\n",
            "exitCode": 0,
            "status": "completed",
        });
        let n = for_client("codex", "tool_use", p).unwrap();
        assert_eq!(n["type"], "tool_call");
        assert_eq!(n["tool"], "shell");
        assert_eq!(n["input"]["command"], "git rev-parse --short HEAD");
        assert_eq!(n["input"]["exit_code"], 0);
        assert_eq!(n["input"]["output"], "516d04a\n");
    }

    #[test]
    fn codex_reasoning_maps_to_text() {
        let p = json!({ "type": "reasoning",
            "content": [{ "type": "reasoning_text", "text": "thinking…" }] });
        let n = for_client("codex", "message", p).unwrap();
        assert_eq!(n["type"], "text");
        assert_eq!(n["content"], "thinking…");
    }

    #[test]
    fn codex_mcp_tool_call_namespaced() {
        let p = json!({ "type": "mcpToolCall", "server": "fs", "tool": "read",
            "arguments": { "path": "/x" } });
        let n = for_client("codex", "tool_use", p).unwrap();
        assert_eq!(n["tool"], "mcp__fs__read");
        assert_eq!(n["input"]["path"], "/x");
    }

    #[test]
    fn codex_unknown_item_dropped() {
        let p = json!({ "type": "contextCompaction", "id": "x" });
        assert_eq!(for_client("codex", "message", p), None);
    }

    #[test]
    fn codex_live_agent_message_broadcasts_text() {
        let p = json!({ "type": "agentMessage", "text": "hi" });
        match to_agent_event("codex", "message", &p) {
            Some(AgentEvent::Text { content, .. }) => assert_eq!(content, "hi"),
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn codex_live_command_broadcasts_tool_call() {
        let p = json!({ "type": "commandExecution", "command": "ls", "status": "completed" });
        match to_agent_event("codex", "tool_use", &p) {
            Some(AgentEvent::ToolCall { tool, .. }) => assert_eq!(tool, "shell"),
            other => panic!("expected ToolCall, got {other:?}"),
        }
    }

    #[test]
    fn claude_live_path_unaffected() {
        let p = json!({ "role": "assistant", "text": "hello" });
        match to_agent_event("claude-code", "message", &p) {
            Some(AgentEvent::Text { content, .. }) => assert_eq!(content, "hello"),
            other => panic!("expected Text, got {other:?}"),
        }
    }
}
