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
                "user" => {
                    let meta = payload.get("meta").and_then(Value::as_bool).unwrap_or(false);
                    Some(AgentEvent::Text {
                        content: format!("▷ User: {text}"),
                        meta,
                        ts,
                        message_id: None,
                        usage: None,
                    })
                }
                "assistant" | "assistant_thinking" => Some(AgentEvent::Text {
                    content: text.to_owned(),
                    meta: false,
                    ts,
                    message_id: payload
                        .get("message_id")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    usage: None,
                }),
                "context_reset" => Some(AgentEvent::ContextReset { ts }),
                "compact_summary" => {
                    Some(AgentEvent::CompactSummary { content: text.to_owned(), ts })
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
            meta: v.get("meta").and_then(Value::as_bool).unwrap_or(false),
            ts,
            message_id: v.get("message_id").and_then(Value::as_str).map(str::to_owned),
            usage: serde_json::from_value(v.get("usage").cloned().unwrap_or(Value::Null)).ok(),
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
        "context_reset" => Some(AgentEvent::ContextReset { ts }),
        _ => None,
    }
}

/// Map a codex app-server / log-tail event payload onto the canonical client
/// shape. Codex stores the raw `item/completed` item (CCT-137); its `type`
/// field is a codex-native `ThreadItem` discriminant (verified against
/// `codex app-server generate-json-schema`, codex-cli 0.135). The `event_type`
/// column (`message` / `tool_use`) is advisory only — we key off the item
/// `type`. Unknown items return `None` (dropped from the conversation).
#[allow(clippy::too_many_lines)]
fn codex(_event_type: &str, payload: &Value) -> Option<Value> {
    match payload.get("type").and_then(Value::as_str).unwrap_or_default() {
        // Rollout JSONL envelopes: text comes from `event_msg` and tool activity
        // from `response_item` — both streams carry the same turns, so splitting
        // them by source is what prevents every message rendering twice.
        "event_msg" => payload.get("payload").and_then(codex_event_msg),
        "response_item" => payload.get("payload").and_then(codex_response_item),
        // Final assistant answer + plan updates → assistant text.
        "agentMessage" | "plan" => {
            let text = payload.get("text").and_then(Value::as_str).unwrap_or_default();
            if text.is_empty() {
                return None;
            }
            Some(json!({ "type": "text", "content": text, "role": "Assistant" }))
        }
        // Model reasoning → text. App-server v2 items carry `content`/`summary`
        // as arrays of plain strings; rollout items use arrays of {type, text}.
        // `codex_text_parts` handles both; fall back to `summary` when the
        // visible reasoning lives there.
        "reasoning" => {
            let text = {
                let c = codex_text_parts(payload.get("content"));
                if c.is_empty() { codex_text_parts(payload.get("summary")) } else { c }
            };
            if text.is_empty() {
                return None;
            }
            Some(json!({ "type": "text", "content": text, "role": "Reasoning" }))
        }
        // Review mode boundaries (CCT-638) → assistant-side note.
        "enteredReviewMode" => {
            let r = payload.get("review").and_then(Value::as_str).unwrap_or_default();
            let content = if r.is_empty() {
                "Entered review mode".to_owned()
            } else {
                format!("Entered review mode: {r}")
            };
            Some(json!({ "type": "text", "content": content, "role": "Review" }))
        }
        "exitedReviewMode" => {
            let r = payload.get("review").and_then(Value::as_str).unwrap_or_default();
            let content = if r.is_empty() {
                "Exited review mode".to_owned()
            } else {
                format!("Review result: {r}")
            };
            Some(json!({ "type": "text", "content": content, "role": "Review" }))
        }
        // A context compaction boundary renders like a /clear cut (CCT-638).
        "contextCompaction" => Some(json!({ "type": "context_reset" })),
        // Sub-agent hand-off activity → a compact status line.
        "subAgentActivity" => {
            let kind = payload.get("kind").and_then(Value::as_str).unwrap_or_default();
            let path = payload.get("agentPath").and_then(Value::as_str).unwrap_or_default();
            Some(json!({ "type": "text", "content": format!("· sub-agent {kind}: {path}") }))
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
        // A dynamic (namespaced) tool call (CCT-638) → tool_call, keeping the
        // `namespace__tool` name so the renderer groups it like an MCP call.
        "dynamicToolCall" => {
            let tool = payload.get("tool").and_then(Value::as_str).unwrap_or_default();
            let name = match payload.get("namespace").and_then(Value::as_str) {
                Some(ns) if !ns.is_empty() => format!("{ns}__{tool}"),
                _ => tool.to_owned(),
            };
            Some(json!({
                "type": "tool_call",
                "tool": name,
                "input": payload.get("arguments").cloned().unwrap_or(Value::Null),
            }))
        }
        // Collaboration hand-off to another agent (CCT-638) → tool_call carrying
        // the delegated prompt/model.
        "collabAgentToolCall" => {
            let tool = payload.get("tool").and_then(Value::as_str).unwrap_or("collab");
            let mut input = json!({});
            if let Some(p) = payload.get("prompt").filter(|v| !v.is_null()) {
                input["prompt"] = p.clone();
            }
            if let Some(m) = payload.get("model").filter(|v| !v.is_null()) {
                input["model"] = m.clone();
            }
            Some(json!({ "type": "tool_call", "tool": tool, "input": input }))
        }
        // Image items (CCT-638): a viewed local image or a generated one.
        "imageView" => Some(json!({
            "type": "tool_call",
            "tool": "view_image",
            "input": { "path": payload.get("path").cloned().unwrap_or(Value::Null) },
        })),
        "imageGeneration" => {
            let mut input = json!({});
            if let Some(p) = payload.get("revisedPrompt").filter(|v| !v.is_null()) {
                input["prompt"] = p.clone();
            }
            if let Some(p) = payload.get("savedPath").filter(|v| !v.is_null()) {
                input["saved_path"] = p.clone();
            }
            Some(json!({ "type": "tool_call", "tool": "image_generation", "input": input }))
        }
        _ => None,
    }
}

/// Join the text of a codex content array, accepting both the app-server v2
/// shape (array of plain strings) and the rollout shape (array of {type, text}).
fn codex_text_parts(content: Option<&Value>) -> String {
    let Some(arr) = content.and_then(Value::as_array) else { return String::new() };
    arr.iter()
        .filter_map(|c| match c {
            Value::String(s) => Some(s.as_str()),
            _ => c.get("text").and_then(Value::as_str),
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Join the `text` fields of a codex content array (`userMessage.content`,
/// `reasoning.content`) into a single string.
fn codex_content_text(content: Option<&Value>) -> String {
    let Some(arr) = content.and_then(Value::as_array) else { return String::new() };
    arr.iter().filter_map(|c| c.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n")
}

/// Map a rollout `event_msg` payload onto the canonical client shape. These are
/// the clean conversational turns: `user_message`/`agent_message` carry a plain
/// `message` string. `token_count` and lifecycle events have no transcript value.
fn codex_event_msg(inner: &Value) -> Option<Value> {
    match inner.get("type").and_then(Value::as_str).unwrap_or_default() {
        "user_message" => {
            let text = inner.get("message").and_then(Value::as_str).unwrap_or_default();
            if text.is_empty() {
                return None;
            }
            Some(json!({ "type": "text", "content": format!("▷ User: {text}") }))
        }
        "agent_message" => {
            let text = inner.get("message").and_then(Value::as_str).unwrap_or_default();
            if text.is_empty() {
                return None;
            }
            Some(json!({ "type": "text", "content": text, "role": "Assistant" }))
        }
        _ => None,
    }
}

/// Map a rollout `response_item` payload (raw `OpenAI` Responses-API item) onto the
/// canonical client shape. Only tool activity is taken here — `message` items are
/// dropped because their text is already surfaced via `event_msg` (see [`codex`]).
fn codex_response_item(inner: &Value) -> Option<Value> {
    match inner.get("type").and_then(Value::as_str).unwrap_or_default() {
        "function_call" => {
            let tool = inner.get("name").and_then(Value::as_str).unwrap_or_default();
            let input = inner
                .get("arguments")
                .and_then(Value::as_str)
                .and_then(|s| serde_json::from_str::<Value>(s).ok())
                .or_else(|| inner.get("arguments").cloned())
                .unwrap_or(Value::Null);
            Some(json!({ "type": "tool_call", "tool": tool, "input": input }))
        }
        "custom_tool_call" => {
            let tool = inner.get("name").and_then(Value::as_str).unwrap_or_default();
            let input = match inner.get("input") {
                Some(Value::String(s)) => json!({ "input": s }),
                Some(other) => other.clone(),
                None => Value::Null,
            };
            Some(json!({ "type": "tool_call", "tool": tool, "input": input }))
        }
        "function_call_output" | "custom_tool_call_output" => Some(
            json!({ "type": "tool_result", "output_summary": codex_output_summary(inner.get("output")) }),
        ),
        "reasoning" => {
            let text = codex_content_text(inner.get("summary"));
            if text.is_empty() {
                return None;
            }
            Some(json!({ "type": "text", "content": text, "role": "Reasoning" }))
        }
        _ => None,
    }
}

/// Flatten a tool-call output into a capped summary string. Outputs are either a
/// plain string or an array of `{type, text}` content parts.
fn codex_output_summary(output: Option<&Value>) -> String {
    let raw = match output {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(_)) => codex_content_text(output),
        Some(other) => other.to_string(),
        None => String::new(),
    };
    raw.chars().take(4000).collect()
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
        "user" => {
            let meta = payload.get("meta").and_then(Value::as_bool).unwrap_or(false);
            Some(json!({ "type": "text", "content": format!("▷ User: {text}"), "meta": meta }))
        }
        "assistant" | "assistant_thinking" => Some(json!({
            "type": "text",
            "content": text,
            "role": "Assistant",
            "message_id": payload.get("message_id"),
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
        "context_reset" => Some(json!({ "type": "context_reset" })),
        "compact_summary" => Some(json!({ "type": "compact_summary", "content": text })),
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
        assert_eq!(n["meta"], false);
    }

    #[test]
    fn daemon_user_meta_flag_flows_through() {
        // Read path preserves the adapter-set `meta` flag.
        let p = json!({ "role": "user", "text": "<task-notification/>", "meta": true });
        let n = for_client("claude-code", "message", p).unwrap();
        assert_eq!(n["meta"], true);

        // Live path lifts the same flag onto AgentEvent::Text.
        let ev = to_agent_event(
            "claude-code",
            "message",
            &json!({ "role": "user", "text": "x", "meta": true }),
        )
        .unwrap();
        match ev {
            AgentEvent::Text { meta, .. } => assert!(meta),
            other => panic!("expected Text, got {other:?}"),
        }
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
    fn codex_legacy_role_text_preview_is_dropped() {
        // CCT-276: the old inventory preview payload `{role,text}` has no codex
        // `type` discriminant, so the codex normalizer drops it → the
        // conversation drawer rendered "No events yet". Documents that bug.
        let p = json!({ "role": "user", "text": "Implement CCT-276 please." });
        assert_eq!(for_client("codex", "message", p), None);
    }

    #[test]
    fn codex_native_preview_survives_normalize() {
        // CCT-276 fix: the inventory now emits the preview as a codex-native
        // `userMessage`, which normalizes to a renderable user line.
        let p = json!({ "type": "userMessage", "content": [
            { "type": "text", "text": "Implement CCT-276 please." }
        ]});
        let n = for_client("codex", "message", p).unwrap();
        assert_eq!(n["type"], "text");
        assert_eq!(n["content"], "▷ User: Implement CCT-276 please.");
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
        let p = json!({ "type": "sleep", "id": "x", "durationMs": 500 });
        assert_eq!(for_client("codex", "message", p), None);
    }

    // --- CCT-638: expanded item fidelity ------------------------------------

    #[test]
    fn codex_reasoning_app_server_string_content() {
        // App-server v2 reasoning items carry `content` as an array of strings.
        let p = json!({ "type": "reasoning", "content": ["First I will list the files."], "summary": [] });
        let n = for_client("codex", "message", p).unwrap();
        assert_eq!(n["type"], "text");
        assert_eq!(n["content"], "First I will list the files.");
        assert_eq!(n["role"], "Reasoning");
    }

    #[test]
    fn codex_reasoning_falls_back_to_summary() {
        let p = json!({ "type": "reasoning", "content": [], "summary": ["short recap"] });
        let n = for_client("codex", "message", p).unwrap();
        assert_eq!(n["content"], "short recap");
    }

    #[test]
    fn codex_review_modes_map_to_review_text() {
        let entered = json!({ "type": "enteredReviewMode", "review": "check the diff" });
        let n = for_client("codex", "message", entered).unwrap();
        assert_eq!(n["role"], "Review");
        assert_eq!(n["content"], "Entered review mode: check the diff");
        let exited = json!({ "type": "exitedReviewMode", "review": "looks good" });
        let m = for_client("codex", "message", exited).unwrap();
        assert_eq!(m["content"], "Review result: looks good");
    }

    #[test]
    fn codex_context_compaction_maps_to_context_reset() {
        let p = json!({ "type": "contextCompaction", "id": "x" });
        let n = for_client("codex", "message", p.clone()).unwrap();
        assert_eq!(n["type"], "context_reset");
        // Live path lifts it onto the ContextReset broadcast event.
        match to_agent_event("codex", "message", &p) {
            Some(AgentEvent::ContextReset { .. }) => {}
            other => panic!("expected ContextReset, got {other:?}"),
        }
    }

    #[test]
    fn codex_dynamic_tool_call_namespaced() {
        let p = json!({ "type": "dynamicToolCall", "namespace": "browser", "tool": "navigate",
            "arguments": { "url": "https://example.com" }, "status": "completed" });
        let n = for_client("codex", "tool_use", p).unwrap();
        assert_eq!(n["type"], "tool_call");
        assert_eq!(n["tool"], "browser__navigate");
        assert_eq!(n["input"]["url"], "https://example.com");
    }

    #[test]
    fn codex_collab_agent_tool_call_carries_prompt() {
        let p = json!({ "type": "collabAgentToolCall", "tool": "delegate",
            "prompt": "do the thing", "model": "gpt-5-codex", "status": "completed" });
        let n = for_client("codex", "tool_use", p).unwrap();
        assert_eq!(n["tool"], "delegate");
        assert_eq!(n["input"]["prompt"], "do the thing");
        assert_eq!(n["input"]["model"], "gpt-5-codex");
    }

    #[test]
    fn codex_image_items_map_to_tool_calls() {
        let view = json!({ "type": "imageView", "path": "/repo/diagram.png" });
        let n = for_client("codex", "tool_use", view).unwrap();
        assert_eq!(n["tool"], "view_image");
        assert_eq!(n["input"]["path"], "/repo/diagram.png");
        let generated = json!({ "type": "imageGeneration", "revisedPrompt": "a cat", "savedPath": "/tmp/cat.png" });
        let m = for_client("codex", "tool_use", generated).unwrap();
        assert_eq!(m["tool"], "image_generation");
        assert_eq!(m["input"]["prompt"], "a cat");
        assert_eq!(m["input"]["saved_path"], "/tmp/cat.png");
    }

    #[test]
    fn codex_sub_agent_activity_maps_to_status_line() {
        let p = json!({ "type": "subAgentActivity", "kind": "started",
            "agentPath": "reviewer", "agentThreadId": "t2" });
        let n = for_client("codex", "message", p).unwrap();
        assert_eq!(n["content"], "· sub-agent started: reviewer");
    }

    // --- codex rollout envelopes (CCT-633); shapes captured from codex 0.144.1 ---

    #[test]
    fn codex_event_msg_user_message_maps_to_user_text() {
        let p = json!({ "type": "event_msg",
            "payload": { "type": "user_message", "message": "do a thing", "text_elements": [] } });
        let n = for_client("codex", "message", p).unwrap();
        assert_eq!(n["type"], "text");
        assert_eq!(n["content"], "▷ User: do a thing");
    }

    #[test]
    fn codex_event_msg_agent_message_maps_to_assistant_text() {
        let p = json!({ "type": "event_msg",
            "payload": { "type": "agent_message", "message": "the answer", "phase": "final_answer" } });
        let n = for_client("codex", "message", p).unwrap();
        assert_eq!(n["content"], "the answer");
        assert_eq!(n["role"], "Assistant");
    }

    #[test]
    fn codex_response_item_message_dropped_to_avoid_dupe() {
        // The assistant text is surfaced via event_msg; the raw response_item
        // message must NOT render a second copy.
        let p = json!({ "type": "response_item", "payload": {
            "type": "message", "role": "assistant",
            "content": [{ "type": "output_text", "text": "the answer" }] } });
        assert_eq!(for_client("codex", "message", p), None);
    }

    #[test]
    fn codex_response_item_custom_tool_call_maps_to_tool_call() {
        let p = json!({ "type": "response_item", "payload": {
            "type": "custom_tool_call", "name": "exec", "call_id": "call_a",
            "input": "const r = await tools.exec_command({cmd:\"ls\"});" } });
        let n = for_client("codex", "tool_use", p).unwrap();
        assert_eq!(n["type"], "tool_call");
        assert_eq!(n["tool"], "exec");
        assert_eq!(n["input"]["input"], "const r = await tools.exec_command({cmd:\"ls\"});");
    }

    #[test]
    fn codex_response_item_function_call_parses_arguments() {
        let p = json!({ "type": "response_item", "payload": {
            "type": "function_call", "name": "wait", "call_id": "call_b",
            "arguments": "{\"cell_id\":\"5\",\"yield_time_ms\":1000}" } });
        let n = for_client("codex", "tool_use", p).unwrap();
        assert_eq!(n["tool"], "wait");
        assert_eq!(n["input"]["cell_id"], "5");
        assert_eq!(n["input"]["yield_time_ms"], 1000);
    }

    #[test]
    fn codex_response_item_tool_output_maps_to_tool_result() {
        let arr = json!({ "type": "response_item", "payload": {
            "type": "custom_tool_call_output", "call_id": "call_a",
            "output": [{ "type": "input_text", "text": "done\n" }, { "type": "input_text", "text": "Cargo.toml\n" }] } });
        let n = for_client("codex", "tool_use", arr).unwrap();
        assert_eq!(n["type"], "tool_result");
        assert_eq!(n["output_summary"], "done\n\nCargo.toml\n");

        let str_out = json!({ "type": "response_item", "payload": {
            "type": "function_call_output", "call_id": "call_b", "output": "running\n" } });
        let m = for_client("codex", "tool_use", str_out).unwrap();
        assert_eq!(m["output_summary"], "running\n");
    }

    #[test]
    fn codex_encrypted_reasoning_dropped() {
        let p = json!({ "type": "response_item", "payload": {
            "type": "reasoning", "id": "rs_1", "summary": [], "encrypted_content": "gAAAA" } });
        assert_eq!(for_client("codex", "message", p), None);
    }

    #[test]
    fn codex_token_count_envelope_dropped_from_transcript() {
        let p = json!({ "type": "event_msg", "payload": {
            "type": "token_count", "info": { "total_token_usage": { "total_tokens": 1 } } } });
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

    #[test]
    fn context_reset_maps_on_both_paths() {
        // CCT-158: a /clear or /compact boundary surfaces as a dedicated event
        // on both the live broadcast and the historical read paths.
        let p = json!({ "role": "context_reset", "text": "x", "session_id": "sess-2" });
        match to_agent_event("claude-code", "message", &p) {
            Some(AgentEvent::ContextReset { .. }) => {}
            other => panic!("expected ContextReset, got {other:?}"),
        }
        let n = for_client("claude-code", "message", p).unwrap();
        assert_eq!(n.get("type").and_then(|v| v.as_str()), Some("context_reset"));
    }

    #[test]
    fn compact_summary_maps_on_both_paths() {
        // CCT-159: a /compact summary carries text and surfaces as a dedicated
        // compact event on both the live and historical read paths.
        let p = json!({ "role": "compact_summary", "text": "the summary" });
        match to_agent_event("claude-code", "message", &p) {
            Some(AgentEvent::CompactSummary { content, .. }) => assert_eq!(content, "the summary"),
            other => panic!("expected CompactSummary, got {other:?}"),
        }
        let n = for_client("claude-code", "message", p).unwrap();
        assert_eq!(n.get("type").and_then(|v| v.as_str()), Some("compact_summary"));
        assert_eq!(n.get("content").and_then(|v| v.as_str()), Some("the summary"));
    }
}
