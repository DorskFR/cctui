//! ACP `session/update` JSON → [`AdapterEvent`] payloads, in both dialects.
//!
//! Payloads carry the canonical client shape (`type`/`content`) and the
//! claude-daemon shape (`role`/`text`) at once, as `adapters/opencode/normalize.rs`
//! does, so the server passes them through and needs no `acp` arm.
//!
//! The input is the wire JSON of one update (`{"sessionUpdate": "…", …}`),
//! not an SDK type: the SDK stays inside `connection.rs`, and a normalizer
//! test is a JSON literal.

use std::collections::HashMap;

use cctui_proto::adapter::{AdapterEvent, PermissionMode};
use serde_json::{Value, json};

use super::modes::ModeTable;

const TOOL_OUTPUT_CAP: usize = 4000;

/// What one update turned into, before it is addressed to a session.
#[derive(Debug, Clone, PartialEq)]
pub enum Out {
    Message(Value),
    ToolUse(Value),
    /// The agent switched mode; `posture` is the cctui reading when known.
    Mode {
        agent_mode: String,
        posture: Option<PermissionMode>,
    },
    /// The agent renamed the session.
    Title(String),
    /// Cumulative context usage; `cost_usd` only when the agent priced it.
    Usage {
        used: u64,
        size: u64,
        cost_usd: Option<f64>,
    },
    /// The config options changed (model, effort, mode, …).
    ConfigOptions(Vec<Value>),
}

#[derive(Debug, Default)]
struct Chunk {
    message_id: Option<String>,
    text: String,
}

#[derive(Debug, Default)]
struct ToolState {
    name: String,
    call_emitted: bool,
    result_emitted: bool,
}

/// Per-session streaming state.
///
/// Open message/thought chunks and the tool calls seen so far. A chunk is
/// emitted as one row when its `messageId` changes, when a non-chunk update
/// arrives, or at [`Self::flush`].
#[derive(Debug, Default)]
pub struct Coalescer {
    message: Option<Chunk>,
    thought: Option<Chunk>,
    user: Option<Chunk>,
    tools: HashMap<String, ToolState>,
    /// Counts updates so a user line without a message id still hashes apart
    /// from a repeat of the same prose.
    seq: u64,
}

impl Coalescer {
    /// Map one update. Returns in emission order.
    pub fn update(&mut self, update: &Value) -> Vec<Out> {
        self.seq += 1;
        let kind = update.get("sessionUpdate").and_then(Value::as_str).unwrap_or_default();
        match kind {
            "agent_message_chunk" => self.chunk(Slot::Message, update),
            "agent_thought_chunk" => self.chunk(Slot::Thought, update),
            "user_message_chunk" => self.chunk(Slot::User, update),
            "tool_call" => {
                let mut out = self.flush();
                out.extend(self.tool_call(update));
                out
            }
            "tool_call_update" => {
                let mut out = self.flush();
                out.extend(self.tool_call_update(update));
                out
            }
            "plan" => {
                let mut out = self.flush();
                out.extend(plan(update, self.seq));
                out
            }
            "current_mode_update" => {
                let agent_mode =
                    update.get("currentModeId").and_then(Value::as_str).unwrap_or_default();
                vec![Out::Mode { agent_mode: agent_mode.to_owned(), posture: None }]
            }
            "session_info_update" => update
                .get("title")
                .and_then(Value::as_str)
                .filter(|t| !t.trim().is_empty())
                .map(|t| vec![Out::Title(t.to_owned())])
                .unwrap_or_default(),
            "usage_update" => vec![Out::Usage {
                used: update.get("used").and_then(Value::as_u64).unwrap_or_default(),
                size: update.get("size").and_then(Value::as_u64).unwrap_or_default(),
                cost_usd: update.get("cost").and_then(cost_usd),
            }],
            "config_option_update" => vec![Out::ConfigOptions(
                update.get("configOptions").and_then(Value::as_array).cloned().unwrap_or_default(),
            )],
            _ => Vec::new(),
        }
    }

    /// Emit whatever is still streaming. Call at turn end.
    pub fn flush(&mut self) -> Vec<Out> {
        let mut out = Vec::new();
        for slot in [Slot::User, Slot::Thought, Slot::Message] {
            if let Some(chunk) = self.slot(slot).take() {
                out.extend(emit_chunk(slot, &chunk, self.seq));
            }
        }
        out
    }

    const fn slot(&mut self, slot: Slot) -> &mut Option<Chunk> {
        match slot {
            Slot::Message => &mut self.message,
            Slot::Thought => &mut self.thought,
            Slot::User => &mut self.user,
        }
    }

    fn chunk(&mut self, slot: Slot, update: &Value) -> Vec<Out> {
        let Some(text) = update.pointer("/content/text").and_then(Value::as_str) else {
            return Vec::new();
        };
        let message_id = update.get("messageId").and_then(Value::as_str).map(str::to_owned);
        let seq = self.seq;
        let open = self.slot(slot);
        let mut out = Vec::new();
        if let Some(current) = open.as_ref()
            && current.message_id != message_id
            && let Some(done) = open.take()
        {
            out.extend(emit_chunk(slot, &done, seq));
        }
        let chunk = open.get_or_insert_with(|| Chunk { message_id, text: String::new() });
        chunk.text.push_str(text);
        out
    }

    fn tool_call(&mut self, update: &Value) -> Vec<Out> {
        let id = tool_id(update);
        let name = tool_name(update);
        let state = self.tools.entry(id.clone()).or_default();
        state.name.clone_from(&name);
        let mut out = Vec::new();
        if !state.call_emitted {
            state.call_emitted = true;
            out.push(Out::ToolUse(tool_call_payload(&name, update, &id)));
        }
        out.extend(self.result_if_final(&id, update));
        out
    }

    fn tool_call_update(&mut self, update: &Value) -> Vec<Out> {
        let id = tool_id(update);
        let mut out = Vec::new();
        let known = self.tools.contains_key(&id);
        let state = self.tools.entry(id.clone()).or_default();
        if !known || !state.call_emitted {
            // The agent skipped the initial `tool_call`; synthesize it.
            state.name = tool_name(update);
            state.call_emitted = true;
            out.push(Out::ToolUse(tool_call_payload(&state.name.clone(), update, &id)));
        }
        out.extend(self.result_if_final(&id, update));
        out
    }

    fn result_if_final(&mut self, id: &str, update: &Value) -> Vec<Out> {
        let status = update.get("status").and_then(Value::as_str).unwrap_or_default();
        let error = match status {
            "completed" => false,
            "failed" => true,
            _ => return Vec::new(),
        };
        let Some(state) = self.tools.get_mut(id) else { return Vec::new() };
        if state.result_emitted {
            return Vec::new();
        }
        state.result_emitted = true;
        vec![Out::ToolUse(tool_result_payload(update, error, id))]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    Message,
    Thought,
    User,
}

fn emit_chunk(slot: Slot, chunk: &Chunk, seq: u64) -> Option<Out> {
    let text = chunk.text.as_str();
    if text.trim().is_empty() {
        return None;
    }
    let message_id = chunk.message_id.as_deref();
    Some(Out::Message(match slot {
        Slot::Message => json!({
            "type": "text",
            "content": text,
            "role": "assistant",
            "text": text,
            "message_id": message_id,
        }),
        Slot::Thought => json!({
            "type": "text",
            "content": text,
            "role": "assistant_thinking",
            "text": text,
            "message_id": message_id,
        }),
        Slot::User => json!({
            "type": "text",
            "content": format!("▷ User: {text}"),
            "role": "user",
            "text": text,
            "meta": false,
            "line_id": message_id.map_or_else(|| format!("acp-user-{seq}"), str::to_owned),
        }),
    }))
}

fn tool_id(update: &Value) -> String {
    update.get("toolCallId").and_then(Value::as_str).unwrap_or("tool").to_owned()
}

/// `name` when the agent sends one, else `kind`, else the title.
fn tool_name(update: &Value) -> String {
    update
        .get("name")
        .and_then(Value::as_str)
        .filter(|n| !n.is_empty())
        .or_else(|| update.get("kind").and_then(Value::as_str).filter(|k| *k != "other"))
        .or_else(|| update.get("title").and_then(Value::as_str))
        .unwrap_or("tool")
        .to_owned()
}

fn tool_call_payload(name: &str, update: &Value, call_id: &str) -> Value {
    let input = match update.get("rawInput") {
        Some(raw) if !raw.is_null() => raw.clone(),
        _ => {
            let mut input = serde_json::Map::new();
            if let Some(title) = update.get("title").and_then(Value::as_str) {
                input.insert("title".to_owned(), Value::String(title.to_owned()));
            }
            if let Some(kind) = update.get("kind").and_then(Value::as_str) {
                input.insert("kind".to_owned(), Value::String(kind.to_owned()));
            }
            Value::Object(input)
        }
    };
    json!({
        "type": "tool_call",
        "tool": name,
        "input": input,
        "id": call_id,
        "tool_use_id": call_id,
        "title": update.get("title").and_then(Value::as_str),
    })
}

/// The result text: content blocks first, then a diff summary, then the raw
/// output dumped as JSON.
fn tool_output(update: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();
    for block in update.get("content").and_then(Value::as_array).into_iter().flatten() {
        match block.get("type").and_then(Value::as_str) {
            Some("content") => {
                if let Some(text) = block.pointer("/content/text").and_then(Value::as_str) {
                    parts.push(text.to_owned());
                }
            }
            Some("diff") => {
                let path = block.get("path").and_then(Value::as_str).unwrap_or("?");
                let new_text = block.get("newText").and_then(Value::as_str).unwrap_or_default();
                parts.push(format!("diff {path} ({} line(s))", new_text.lines().count()));
            }
            Some("terminal") => {
                if let Some(id) = block.get("terminalId").and_then(Value::as_str) {
                    parts.push(format!("terminal {id}"));
                }
            }
            _ => {}
        }
    }
    if parts.is_empty()
        && let Some(raw) = update.get("rawOutput").filter(|v| !v.is_null())
    {
        parts.push(match raw {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        });
    }
    parts.join("\n")
}

fn tool_result_payload(update: &Value, error: bool, call_id: &str) -> Value {
    let output = tool_output(update);
    let capped: String = output.chars().take(TOOL_OUTPUT_CAP).collect();
    json!({
        "type": "tool_result",
        "output_summary": capped,
        "kind": "tool_result",
        "content": capped,
        "is_error": error,
        "error": error,
        "tool_use_id": call_id,
    })
}

/// The agent plan as the `update_plan` tool call every client already
/// renders for codex and claude: `{step, status}` per entry.
fn plan(update: &Value, seq: u64) -> Option<Out> {
    let entries = update.get("entries").and_then(Value::as_array)?;
    let steps: Vec<Value> = entries
        .iter()
        .map(|e| {
            json!({
                "step": e.get("content").and_then(Value::as_str).unwrap_or_default(),
                "status": e.get("status").and_then(Value::as_str).unwrap_or("pending"),
                "priority": e.get("priority").and_then(Value::as_str),
            })
        })
        .collect();
    let id = format!("acp-plan-{seq}");
    Some(Out::ToolUse(json!({
        "type": "tool_call",
        "tool": "update_plan",
        "input": { "plan": steps },
        "id": id,
        "tool_use_id": id,
    })))
}

fn cost_usd(cost: &Value) -> Option<f64> {
    let currency = cost.get("currency").and_then(Value::as_str).unwrap_or("USD");
    if !currency.eq_ignore_ascii_case("usd") {
        return None;
    }
    cost.get("amount").and_then(Value::as_f64)
}

/// Read a `current_mode_update` back into a cctui posture through the row's
/// table.
#[must_use]
pub fn posture(table: &ModeTable, agent_mode: &str) -> Option<PermissionMode> {
    table.posture_of(agent_mode)
}

/// `session/request_permission` → the prompt the clients render. The request
/// id is the tool call id, which is what the agent correlates on too.
#[must_use]
pub fn permission_request(local_id: &str, request_id: &str, tool_call: &Value) -> AdapterEvent {
    AdapterEvent::PermissionRequest {
        local_id: local_id.to_owned(),
        request_id: request_id.to_owned(),
        tool: tool_name(tool_call),
        input: match tool_call.get("rawInput") {
            Some(raw) if !raw.is_null() => raw.clone(),
            _ => json!({ "title": tool_call.get("title").and_then(Value::as_str) }),
        },
    }
}

/// The `stopReason` line shown when a turn ended for a reason other than the
/// model finishing.
#[must_use]
pub fn stop_detail(stop_reason: &str) -> Option<String> {
    match stop_reason {
        "end_turn" => None,
        "cancelled" => Some("turn interrupted".to_owned()),
        "max_tokens" => Some("turn stopped: token limit reached".to_owned()),
        "max_turn_requests" => Some("turn stopped: request limit reached".to_owned()),
        "refusal" => Some("turn stopped: the model refused".to_owned()),
        other => Some(format!("turn stopped: {other}")),
    }
}

/// Address pure outputs to a session.
#[must_use]
pub fn address(local_id: &str, out: Out) -> Option<AdapterEvent> {
    match out {
        Out::Message(payload) => {
            Some(AdapterEvent::Message { local_id: local_id.to_owned(), payload, turn_id: None })
        }
        Out::ToolUse(payload) => {
            Some(AdapterEvent::ToolUse { local_id: local_id.to_owned(), payload })
        }
        Out::Mode { agent_mode, posture } => Some(AdapterEvent::Status {
            local_id: local_id.to_owned(),
            tempo: None,
            state: None,
            detail: Some(format!("mode: {agent_mode}")),
            activity: None,
            name: None,
            intent: None,
            model: None,
            effort: None,
            permission_mode: posture,
            children: Vec::new(),
        }),
        Out::Title(name) => Some(AdapterEvent::Status {
            local_id: local_id.to_owned(),
            tempo: None,
            state: None,
            detail: None,
            activity: None,
            name: Some(name),
            intent: None,
            model: None,
            effort: None,
            permission_mode: None,
            children: Vec::new(),
        }),
        Out::Usage { .. } | Out::ConfigOptions(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(kind: &str, text: &str, message_id: Option<&str>) -> Value {
        json!({
            "sessionUpdate": kind,
            "content": { "type": "text", "text": text },
            "messageId": message_id,
        })
    }

    #[test]
    fn message_chunks_coalesce_by_message_id_into_one_dual_dialect_row() {
        let mut c = Coalescer::default();
        assert!(c.update(&chunk("agent_message_chunk", "Hel", Some("m1"))).is_empty());
        assert!(c.update(&chunk("agent_message_chunk", "lo", Some("m1"))).is_empty());
        let out = c.update(&chunk("agent_message_chunk", "Next", Some("m2")));
        assert_eq!(out.len(), 1);
        let Out::Message(p) = &out[0] else { panic!("{out:?}") };
        assert_eq!(p["type"], "text");
        assert_eq!(p["content"], "Hello");
        assert_eq!(p["role"], "assistant");
        assert_eq!(p["text"], "Hello");
        assert_eq!(p["message_id"], "m1");
        let tail = c.flush();
        assert_eq!(tail.len(), 1);
        let Out::Message(p) = &tail[0] else { panic!("{tail:?}") };
        assert_eq!(p["content"], "Next");
        assert!(c.flush().is_empty());
    }

    #[test]
    fn chunks_without_a_message_id_coalesce_until_something_else_arrives() {
        let mut c = Coalescer::default();
        c.update(&chunk("agent_message_chunk", "a", None));
        c.update(&chunk("agent_message_chunk", "b", None));
        let out = c.update(&json!({
            "sessionUpdate": "tool_call", "toolCallId": "t1", "title": "ls", "kind": "execute"
        }));
        assert_eq!(out.len(), 2, "{out:?}");
        let Out::Message(p) = &out[0] else { panic!("{out:?}") };
        assert_eq!(p["content"], "ab");
        assert!(p["message_id"].is_null());
        let Out::ToolUse(t) = &out[1] else { panic!("{out:?}") };
        assert_eq!(t["type"], "tool_call");
    }

    #[test]
    fn thought_chunks_become_assistant_thinking() {
        let mut c = Coalescer::default();
        c.update(&chunk("agent_thought_chunk", "hmm", Some("m1")));
        let out = c.flush();
        let Out::Message(p) = &out[0] else { panic!("{out:?}") };
        assert_eq!(p["role"], "assistant_thinking");
        assert_eq!(p["content"], "hmm");
    }

    #[test]
    fn user_chunks_are_prefixed_in_the_canonical_field_only() {
        let mut c = Coalescer::default();
        c.update(&chunk("user_message_chunk", "do it", None));
        let out = c.flush();
        let Out::Message(p) = &out[0] else { panic!("{out:?}") };
        assert_eq!(p["content"], "▷ User: do it");
        assert_eq!(p["text"], "do it");
        assert_eq!(p["role"], "user");
        assert!(p["line_id"].as_str().unwrap().starts_with("acp-user-"));
    }

    #[test]
    fn blank_chunks_emit_nothing() {
        let mut c = Coalescer::default();
        c.update(&chunk("agent_message_chunk", "  \n", Some("m1")));
        assert!(c.flush().is_empty());
    }

    #[test]
    fn a_tool_call_then_its_completion_yield_call_and_result_once() {
        let mut c = Coalescer::default();
        let call = json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "call_1",
            "title": "Read a.rs",
            "name": "read_file",
            "kind": "read",
            "status": "pending",
            "rawInput": { "path": "a.rs" },
        });
        let out = c.update(&call);
        assert_eq!(out.len(), 1);
        let Out::ToolUse(t) = &out[0] else { panic!("{out:?}") };
        assert_eq!(t["tool"], "read_file");
        assert_eq!(t["input"]["path"], "a.rs");
        assert_eq!(t["tool_use_id"], "call_1");
        assert_eq!(t["id"], "call_1");
        assert_eq!(t["title"], "Read a.rs");

        let running = json!({
            "sessionUpdate": "tool_call_update", "toolCallId": "call_1", "status": "in_progress"
        });
        assert!(c.update(&running).is_empty());

        let done = json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "call_1",
            "status": "completed",
            "content": [{ "type": "content", "content": { "type": "text", "text": "fn main() {}" } }],
        });
        let out = c.update(&done);
        assert_eq!(out.len(), 1);
        let Out::ToolUse(r) = &out[0] else { panic!("{out:?}") };
        assert_eq!(r["type"], "tool_result");
        assert_eq!(r["output_summary"], "fn main() {}");
        assert_eq!(r["content"], "fn main() {}");
        assert_eq!(r["kind"], "tool_result");
        assert_eq!(r["is_error"], false);
        assert_eq!(r["tool_use_id"], "call_1");
        assert!(c.update(&done).is_empty(), "a repeated completion is not a second result");
    }

    #[test]
    fn a_failed_update_without_a_prior_call_synthesizes_the_call() {
        let mut c = Coalescer::default();
        let out = c.update(&json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "call_9",
            "kind": "execute",
            "status": "failed",
            "rawOutput": { "exit": 1, "stderr": "denied" },
        }));
        assert_eq!(out.len(), 2, "{out:?}");
        let Out::ToolUse(call) = &out[0] else { panic!("{out:?}") };
        assert_eq!(call["tool"], "execute");
        let Out::ToolUse(res) = &out[1] else { panic!("{out:?}") };
        assert_eq!(res["is_error"], true);
        assert!(res["output_summary"].as_str().unwrap().contains("denied"));
    }

    #[test]
    fn unknown_input_is_dumped_from_the_title_and_kind() {
        let mut c = Coalescer::default();
        let out = c.update(&json!({
            "sessionUpdate": "tool_call", "toolCallId": "t", "title": "Search", "kind": "search"
        }));
        let Out::ToolUse(t) = &out[0] else { panic!("{out:?}") };
        assert_eq!(t["tool"], "search");
        assert_eq!(t["input"]["title"], "Search");
    }

    #[test]
    fn a_diff_result_is_summarised_and_output_is_capped() {
        let mut c = Coalescer::default();
        c.update(&json!({ "sessionUpdate": "tool_call", "toolCallId": "d", "title": "Edit" }));
        let out = c.update(&json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "d",
            "status": "completed",
            "content": [{ "type": "diff", "path": "/w/a.rs", "oldText": "a", "newText": "b\nc" }],
        }));
        let Out::ToolUse(r) = &out[0] else { panic!("{out:?}") };
        assert_eq!(r["output_summary"], "diff /w/a.rs (2 line(s))");

        c.update(&json!({ "sessionUpdate": "tool_call", "toolCallId": "big", "title": "Run" }));
        let out = c.update(&json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "big",
            "status": "completed",
            "rawOutput": "x".repeat(TOOL_OUTPUT_CAP + 10),
        }));
        let Out::ToolUse(r) = &out[0] else { panic!("{out:?}") };
        assert_eq!(r["output_summary"].as_str().unwrap().len(), TOOL_OUTPUT_CAP);
    }

    #[test]
    fn a_plan_is_the_update_plan_tool_call_with_step_and_status() {
        let mut c = Coalescer::default();
        let out = c.update(&json!({
            "sessionUpdate": "plan",
            "entries": [
                { "content": "Read the code", "priority": "high", "status": "completed" },
                { "content": "Write the fix", "priority": "medium", "status": "in_progress" },
            ],
        }));
        assert_eq!(out.len(), 1);
        let Out::ToolUse(t) = &out[0] else { panic!("{out:?}") };
        assert_eq!(t["tool"], "update_plan");
        assert_eq!(t["input"]["plan"][0]["step"], "Read the code");
        assert_eq!(t["input"]["plan"][0]["status"], "completed");
        assert_eq!(t["input"]["plan"][1]["status"], "in_progress");
        assert!(t["tool_use_id"].as_str().unwrap().starts_with("acp-plan-"));
    }

    #[test]
    fn a_plan_flushes_the_open_message_first() {
        let mut c = Coalescer::default();
        c.update(&chunk("agent_message_chunk", "Plan:", Some("m1")));
        let out = c.update(&json!({ "sessionUpdate": "plan", "entries": [] }));
        assert_eq!(out.len(), 2);
        assert!(matches!(&out[0], Out::Message(_)));
        assert!(matches!(&out[1], Out::ToolUse(_)));
    }

    #[test]
    fn mode_title_usage_and_config_updates_map_to_their_outputs() {
        let mut c = Coalescer::default();
        assert_eq!(
            c.update(&json!({ "sessionUpdate": "current_mode_update", "currentModeId": "yolo" })),
            [Out::Mode { agent_mode: "yolo".to_owned(), posture: None }]
        );
        assert_eq!(
            c.update(&json!({ "sessionUpdate": "session_info_update", "title": "Fix the bug" })),
            [Out::Title("Fix the bug".to_owned())]
        );
        assert!(
            c.update(&json!({ "sessionUpdate": "session_info_update", "title": " " })).is_empty()
        );
        assert_eq!(
            c.update(&json!({
                "sessionUpdate": "usage_update", "used": 1200, "size": 200_000,
                "cost": { "amount": 0.0042, "currency": "USD" }
            })),
            [Out::Usage { used: 1200, size: 200_000, cost_usd: Some(0.0042) }]
        );
        assert_eq!(
            c.update(&json!({
                "sessionUpdate": "usage_update", "used": 1, "size": 2,
                "cost": { "amount": 3.0, "currency": "EUR" }
            })),
            [Out::Usage { used: 1, size: 2, cost_usd: None }]
        );
        let out = c.update(&json!({
            "sessionUpdate": "config_option_update",
            "configOptions": [{ "id": "model", "category": "model" }],
        }));
        assert!(matches!(&out[0], Out::ConfigOptions(v) if v.len() == 1));
        assert!(c.update(&json!({ "sessionUpdate": "available_commands_update" })).is_empty());
    }

    #[test]
    fn a_mode_update_is_read_back_through_the_row_table() {
        use super::super::modes::GEMINI_STYLE;
        assert_eq!(posture(&GEMINI_STYLE, "autoEdit"), Some(PermissionMode::Auto));
        assert_eq!(posture(&GEMINI_STYLE, "mystery"), None);
        let evt = address(
            "s1",
            Out::Mode { agent_mode: "autoEdit".to_owned(), posture: Some(PermissionMode::Auto) },
        )
        .unwrap();
        let AdapterEvent::Status { detail, permission_mode, .. } = evt else { panic!() };
        assert_eq!(detail.as_deref(), Some("mode: autoEdit"));
        assert_eq!(permission_mode, Some(PermissionMode::Auto));
    }

    #[test]
    fn a_permission_request_names_the_tool_and_carries_its_input() {
        let evt = permission_request(
            "s1",
            "call_3",
            &json!({ "toolCallId": "call_3", "title": "Run `rm -rf dist`", "kind": "execute",
                     "rawInput": { "command": "rm -rf dist" } }),
        );
        let AdapterEvent::PermissionRequest { local_id, request_id, tool, input } = evt else {
            panic!()
        };
        assert_eq!(local_id, "s1");
        assert_eq!(request_id, "call_3");
        assert_eq!(tool, "execute");
        assert_eq!(input["command"], "rm -rf dist");
    }

    #[test]
    fn stop_reasons_other_than_end_turn_get_a_detail_line() {
        assert_eq!(stop_detail("end_turn"), None);
        assert_eq!(stop_detail("cancelled").as_deref(), Some("turn interrupted"));
        assert!(stop_detail("refusal").unwrap().contains("refused"));
        assert!(stop_detail("weird").unwrap().contains("weird"));
    }

    #[test]
    fn usage_and_config_outputs_are_not_wire_events() {
        assert!(address("s1", Out::Usage { used: 1, size: 2, cost_usd: None }).is_none());
        assert!(address("s1", Out::ConfigOptions(vec![])).is_none());
        let evt = address("s1", Out::Title("t".to_owned())).unwrap();
        assert!(matches!(evt, AdapterEvent::Status { name: Some(n), .. } if n == "t"));
    }
}
