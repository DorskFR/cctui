//! Parses raw JSONL transcript lines (from Claude Code's session.jsonl) into AgentEvents.
//!
//! Ported from channel/src/transcript.ts.

use cctui_proto::ws::AgentEvent;

const SKIP_TYPES: &[&str] = &[
    "file-history-snapshot",
    "queue-operation",
    "system",
    "command",
    "progress",
    "metadata",
    "config_change",
];

pub struct UsageData {
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cost_usd: f64,
}

/// Extract readable text from a tool_result content value (string, array of blocks, or object).
fn extract_tool_result_content(content: &serde_json::Value) -> String {
    match content {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(arr) => arr
            .iter()
            .filter_map(|item| {
                let obj = item.as_object()?;
                let block_type = obj.get("type").and_then(|t| t.as_str()).unwrap_or("");
                if block_type == "text" {
                    obj.get("text").and_then(|t| t.as_str()).map(String::from)
                } else if let Some(title) = obj.get("title").and_then(|t| t.as_str()) {
                    let link = obj.get("link").and_then(|l| l.as_str()).unwrap_or("");
                    if link.is_empty() {
                        Some(title.to_owned())
                    } else {
                        Some(format!("{title} ({link})"))
                    }
                } else {
                    obj.get("text").and_then(|t| t.as_str()).map(String::from)
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        serde_json::Value::Object(obj) => obj
            .get("text")
            .or_else(|| obj.get("message"))
            .or_else(|| obj.get("content"))
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_default(),
        _ => String::new(),
    }
}

fn is_command_artifact(s: &str) -> bool {
    s.contains("</command-name>") || s.contains("<command-")
}

/// Parse a raw JSONL line into zero or more AgentEvents for live broadcast.
pub fn parse_line(line: &str, ts: i64) -> Vec<AgentEvent> {
    let Ok(d) = serde_json::from_str::<serde_json::Value>(line) else {
        return vec![];
    };

    let msg_type = d.get("type").and_then(|t| t.as_str()).unwrap_or("");
    if SKIP_TYPES.contains(&msg_type) {
        return vec![];
    }

    let empty = serde_json::Value::Object(serde_json::Map::new());
    let msg = d.get("message").unwrap_or(&empty);
    let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("");

    if role == "system" {
        return vec![];
    }

    let content = msg.get("content").unwrap_or(&serde_json::Value::Null);

    if role == "user" {
        if let Some(text) = content.as_str() {
            if text.is_empty() || is_command_artifact(text) {
                return vec![];
            }
            return vec![AgentEvent::Text { content: format!("▷ User: {text}"), ts }];
        }

        if let Some(parts) = content.as_array() {
            let mut events = Vec::new();
            for part in parts {
                let part_type = part.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match part_type {
                    "tool_result" => {
                        let raw =
                            extract_tool_result_content(part.get("content").unwrap_or(&serde_json::Value::Null));
                        if is_command_artifact(&raw) {
                            continue;
                        }
                        let tool_use_id = part
                            .get("tool_use_id")
                            .and_then(|t| t.as_str())
                            .unwrap_or("")
                            .to_owned();
                        let summary: String = raw.chars().take(200).collect();
                        events.push(AgentEvent::ToolResult { tool: tool_use_id, output_summary: summary, ts });
                    }
                    "text" => {
                        let text = part.get("text").and_then(|t| t.as_str()).unwrap_or("");
                        if !text.is_empty() && !is_command_artifact(text) {
                            events.push(AgentEvent::Text {
                                content: format!("▷ User: {text}"),
                                ts,
                            });
                        }
                    }
                    _ => {}
                }
            }
            return events;
        }

        return vec![];
    }

    if role == "assistant" {
        if let Some(parts) = content.as_array() {
            let mut events = Vec::new();
            for part in parts {
                let part_type = part.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match part_type {
                    "text" => {
                        let text = part.get("text").and_then(|t| t.as_str()).unwrap_or("");
                        if !text.is_empty() {
                            events.push(AgentEvent::Text { content: text.to_owned(), ts });
                        }
                    }
                    "tool_use" => {
                        let tool = part
                            .get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("")
                            .to_owned();
                        let input = part
                            .get("input")
                            .cloned()
                            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                        events.push(AgentEvent::ToolCall { tool, input, ts });
                    }
                    _ => {}
                }
            }
            return events;
        }

        if let Some(text) = content.as_str() {
            if !text.is_empty() {
                return vec![AgentEvent::Text { content: text.to_owned(), ts }];
            }
        }
    }

    vec![]
}

/// Parse token usage from a raw JSONL line.
pub fn parse_usage(line: &str) -> Option<UsageData> {
    let d = serde_json::from_str::<serde_json::Value>(line).ok()?;

    // Try message.usage first, then top-level usage
    let usage = d
        .get("message")
        .and_then(|m| m.get("usage"))
        .or_else(|| d.get("usage"))?;

    let get_u64 = |key: &str| usage.get(key).and_then(|v| v.as_u64()).unwrap_or(0);

    let tokens_in = get_u64("input_tokens")
        + get_u64("cache_creation_input_tokens")
        + get_u64("cache_read_input_tokens");
    let tokens_out = get_u64("output_tokens");

    if tokens_in == 0 && tokens_out == 0 {
        return None;
    }

    // Sonnet 3.7 pricing: $3/M input, $15/M output
    let cost_usd = (tokens_in as f64 / 1_000_000.0) * 3.0 + (tokens_out as f64 / 1_000_000.0) * 15.0;

    Some(UsageData { tokens_in, tokens_out, cost_usd })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skip_types_are_ignored() {
        let line = r#"{"type":"system","message":{"role":"assistant","content":"hi"}}"#;
        assert!(parse_line(line, 0).is_empty());

        let line = r#"{"type":"progress","message":{"role":"assistant","content":"hi"}}"#;
        assert!(parse_line(line, 0).is_empty());
    }

    #[test]
    fn system_role_is_ignored() {
        let line = r#"{"type":"message","message":{"role":"system","content":"sys prompt"}}"#;
        assert!(parse_line(line, 0).is_empty());
    }

    #[test]
    fn assistant_text_parsed() {
        let line =
            r#"{"type":"message","message":{"role":"assistant","content":[{"type":"text","text":"Hello!"}]}}"#;
        let events = parse_line(line, 42);
        assert_eq!(events.len(), 1);
        match &events[0] {
            AgentEvent::Text { content, ts } => {
                assert_eq!(content, "Hello!");
                assert_eq!(*ts, 42);
            }
            _ => panic!("expected Text"),
        }
    }

    #[test]
    fn assistant_tool_use_parsed() {
        let line = r#"{"type":"message","message":{"role":"assistant","content":[{"type":"tool_use","name":"Bash","input":{"command":"ls"}}]}}"#;
        let events = parse_line(line, 0);
        assert_eq!(events.len(), 1);
        match &events[0] {
            AgentEvent::ToolCall { tool, input, .. } => {
                assert_eq!(tool, "Bash");
                assert_eq!(input["command"], "ls");
            }
            _ => panic!("expected ToolCall"),
        }
    }

    #[test]
    fn user_tool_result_parsed() {
        let line = r#"{"type":"message","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"abc","content":"file.txt"}]}}"#;
        let events = parse_line(line, 0);
        assert_eq!(events.len(), 1);
        match &events[0] {
            AgentEvent::ToolResult { tool, output_summary, .. } => {
                assert_eq!(tool, "abc");
                assert_eq!(output_summary, "file.txt");
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn command_artifacts_filtered() {
        let line = r#"{"type":"message","message":{"role":"user","content":"<command-name>foo</command-name>"}}"#;
        assert!(parse_line(line, 0).is_empty());
    }

    #[test]
    fn usage_parsed() {
        let line = r#"{"type":"message","message":{"usage":{"input_tokens":100,"output_tokens":50,"cache_read_input_tokens":10}}}"#;
        let usage = parse_usage(line).expect("should have usage");
        assert_eq!(usage.tokens_in, 110);
        assert_eq!(usage.tokens_out, 50);
        assert!(usage.cost_usd > 0.0);
    }

    #[test]
    fn invalid_json_returns_empty() {
        assert!(parse_line("not json", 0).is_empty());
        assert!(parse_usage("not json").is_none());
    }
}
