use cctui_proto::ws::AgentEvent;

use super::state::{ConversationLine, LineKind, ToolCategory, TurnFooter};
use super::transcript;

fn extract_tag_content(text: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    if let Some(start) = text.find(&open)
        && let Some(end) = text[start..].find(&close)
    {
        let content_start = start + open.len();
        return Some(text[content_start..content_start + end].to_string());
    }
    None
}

fn remove_tag_pair(text: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    if let Some(start) = text.find(&open)
        && let Some(end) = text[start..].find(&close)
    {
        let end_pos = start + end + close.len();
        let mut result = text[..start].to_string();
        result.push_str(&text[end_pos..]);
        return remove_tag_pair(&result, tag);
    }
    text.to_string()
}

fn strip_all_tags(text: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;
    for ch in text.chars() {
        if ch == '<' {
            in_tag = true;
        } else if ch == '>' {
            in_tag = false;
        } else if !in_tag {
            result.push(ch);
        }
    }
    result
}

fn strip_ansi_codes(text: &str) -> String {
    let mut result = String::new();
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            while let Some(&c) = chars.peek() {
                chars.next();
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else if ch == '[' {
            let mut temp = chars.clone();
            let is_ansi = temp.peek().is_some_and(|&c| c.is_ascii_digit() || c == ';');
            if is_ansi {
                while let Some(&c) = chars.peek() {
                    chars.next();
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                result.push(ch);
            }
        } else {
            result.push(ch);
        }
    }
    result
}

/// Strip XML tags, ANSI codes, and system noise from user message text.
/// Returns None if the result is empty or only whitespace.
fn clean_user_message(text: &str) -> Option<String> {
    let mut result = remove_tag_pair(text, "system-reminder");
    result = remove_tag_pair(&result, "local-command-caveat");

    let cmd_name = extract_tag_content(&result, "command-name");
    let cmd_args = extract_tag_content(&result, "command-args");
    let cmd_stdout = extract_tag_content(&result, "local-command-stdout");

    result = remove_tag_pair(&result, "command-name");
    result = remove_tag_pair(&result, "command-args");
    result = remove_tag_pair(&result, "local-command-stdout");
    result = strip_all_tags(&result);

    if let Some(ref name) = cmd_name {
        let mut cmd_line = format!("/{name}");
        if let Some(ref args) = cmd_args
            && !args.is_empty()
        {
            cmd_line.push(' ');
            cmd_line.push_str(args);
        }
        if let Some(ref stdout) = cmd_stdout {
            cmd_line.push_str(" → ");
            cmd_line.push_str(stdout);
        }
        result = if result.trim().is_empty() { cmd_line } else { format!("{cmd_line} {result}") };
    }

    result = strip_ansi_codes(&result);
    let trimmed = result.trim();
    if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
}

/// Classifies a stored user turn: peer relay, marker, injected, or human.
fn user_line(text: &str, ts: i64, meta: bool) -> Option<ConversationLine> {
    if let Some(peer) = transcript::parse_peer_message(text) {
        let mut line = ConversationLine::new(LineKind::Peer, ts, peer.body);
        line.peer_from = peer.from;
        line.peer_room = peer.room;
        return Some(line);
    }
    if transcript::parse_room_joined(text).is_some() {
        return Some(ConversationLine::new(LineKind::Marker, ts, "joined a room"));
    }
    if transcript::looks_keepalive_tick(text) {
        return Some(ConversationLine::new(LineKind::Marker, ts, "keep-alive tick"));
    }
    // `meta` is set authoritatively at the adapter layer; `looks_meta` catches
    // the turns it misses.
    if meta || transcript::looks_meta(text) {
        return Some(ConversationLine::new(LineKind::System, ts, text.trim()));
    }
    clean_user_message(text).map(|cleaned| ConversationLine::new(LineKind::User, ts, cleaned))
}

/// Only annotations with something to show become a line.
fn annotation_line(content: &str, ts: i64) -> Option<ConversationLine> {
    let (kind, detail) = transcript::parse_annotation(content);
    match kind {
        "turn_duration" => {
            let ms: u64 = detail.trim().parse().ok()?;
            if ms == 0 {
                return None;
            }
            let mut line = ConversationLine::new(LineKind::Summary, ts, String::new());
            line.footer = Some(TurnFooter { duration_ms: Some(ms), ..TurnFooter::default() });
            Some(line)
        }
        "stop_hook_summary" if !detail.trim().is_empty() => {
            Some(ConversationLine::new(LineKind::Marker, ts, format!("hook: {}", detail.trim())))
        }
        _ => None,
    }
}

fn text_line(event: &AgentEvent) -> Option<ConversationLine> {
    let AgentEvent::Text { content, meta, ts, kind, message_id, usage, turn_id, .. } = event else {
        return None;
    };
    // Streaming emits an empty text event before the populated one.
    if content.trim().is_empty() {
        return None;
    }
    let mut line = match kind.as_deref() {
        Some(k @ ("thinking" | "redacted_thinking")) => ConversationLine::new(
            LineKind::Thinking { redacted: k == "redacted_thinking" },
            *ts,
            content.clone(),
        ),
        // Markers carry no user prefix, so they must be claimed before the
        // assistant fallthrough or they read as assistant prose.
        Some("system_marker" | "queue_op") => {
            ConversationLine::new(LineKind::Marker, *ts, content.clone())
        }
        Some("turn_annotation") => return annotation_line(content, *ts),
        _ if content.starts_with(transcript::USER_PREFIX) => {
            user_line(content[transcript::USER_PREFIX.len()..].trim_start(), *ts, *meta)?
        }
        _ => {
            let mut line = ConversationLine::new(LineKind::Assistant, *ts, content.clone());
            line.footer = usage.as_ref().map(|u| TurnFooter {
                duration_ms: None,
                tokens_in: Some(u.tokens_in),
                tokens_out: Some(u.tokens_out),
                needs_action: false,
            });
            line
        }
    };
    line.message_id.clone_from(message_id);
    line.turn_id = *turn_id;
    Some(line)
}

/// One event, at most one line.
///
/// Events with nothing to show — heartbeats, turn ends, the empty text event
/// that precedes a streamed message — return `None` rather than a blank row.
pub fn agent_event_to_line(event: &AgentEvent) -> Option<ConversationLine> {
    match event {
        AgentEvent::Text { .. } => text_line(event),
        AgentEvent::ToolCall { tool, input, kind, ts, .. } => {
            let mut line = ConversationLine::new(
                LineKind::Tool { category: ToolCategory::of(tool, kind.as_deref()) },
                *ts,
                crate::views::sessions::format_tool_input(tool, input),
            );
            line.tool = Some(tool.clone());
            // Edit/Write inputs become an inline diff at render time.
            if matches!(tool.as_str(), "Edit" | "Write") {
                line.tool_input = Some(input.clone());
            }
            Some(line)
        }
        AgentEvent::ToolResult { tool, output_summary, error, ts, .. } => {
            let kind = LineKind::Result { error: *error };
            let mut line = ConversationLine::new(kind, *ts, output_summary.clone());
            line.tool = Some(tool.clone());
            Some(line)
        }
        AgentEvent::Heartbeat { .. } | AgentEvent::TurnEnd { .. } => None,
        AgentEvent::ContextReset { ts, .. } => {
            Some(ConversationLine::new(LineKind::Reset, *ts, "context reset (/clear)"))
        }
        AgentEvent::CompactSummary { content, ts, .. } => (!content.trim().is_empty())
            .then(|| ConversationLine::new(LineKind::Compact, *ts, content.clone())),
        AgentEvent::TurnSummary { detail, status_category, needs_action, ts, .. } => {
            let text = if detail.trim().is_empty() {
                status_category.as_deref().unwrap_or_default().trim()
            } else {
                detail.trim()
            };
            if text.is_empty() {
                return None;
            }
            let mut line = ConversationLine::new(LineKind::Summary, *ts, text);
            line.footer =
                Some(TurnFooter { needs_action: *needs_action, ..TurnFooter::default() });
            Some(line)
        }
        AgentEvent::Reply { content, ts, turn_id, .. } => {
            if content.trim().is_empty() {
                return None;
            }
            let mut line = ConversationLine::new(LineKind::Reply, *ts, content.clone());
            line.turn_id = *turn_id;
            Some(line)
        }
    }
}

#[cfg(test)]
mod tests {
    use cctui_proto::models::TokenUsage;
    use cctui_proto::ws::AgentEvent;
    use serde_json::json;

    use super::agent_event_to_line;
    use crate::app::state::{ConversationLine, LineKind, ToolCategory};

    fn text(content: &str, kind: Option<&str>) -> AgentEvent {
        AgentEvent::Text {
            content: content.to_owned(),
            meta: false,
            kind: kind.map(str::to_owned),
            operation: None,
            ts: 100,
            message_id: None,
            usage: None,
            seq: Some(1),
            turn_id: None,
        }
    }

    fn line(event: &AgentEvent) -> ConversationLine {
        agent_event_to_line(event).expect("a line")
    }

    #[test]
    fn thinking_is_its_own_kind_and_not_assistant_prose() {
        let ln = line(&text("weighing two parsers", Some("thinking")));
        assert_eq!(ln.kind, LineKind::Thinking { redacted: false });
        assert_eq!(ln.text, "weighing two parsers");
        assert!(ln.collapsible());

        let redacted = line(&text("\u{fffd}", Some("redacted_thinking")));
        assert_eq!(redacted.kind, LineKind::Thinking { redacted: true });
    }

    #[test]
    fn an_empty_text_event_produces_no_line() {
        assert!(agent_event_to_line(&text("   ", None)).is_none());
        assert!(agent_event_to_line(&text("", Some("thinking"))).is_none());
    }

    #[test]
    fn heartbeats_and_turn_ends_produce_no_line() {
        let beat =
            AgentEvent::Heartbeat { tokens_in: 1, tokens_out: 2, cost_usd: 0.1, ts: 1, seq: None };
        assert!(agent_event_to_line(&beat).is_none());
        assert!(agent_event_to_line(&AgentEvent::TurnEnd { ts: 1, seq: None }).is_none());
    }

    #[test]
    fn a_user_turn_keeps_its_prose_and_loses_the_storage_prefix() {
        let ln = line(&text("▷ User: refactor the parser", None));
        assert_eq!(ln.kind, LineKind::User);
        assert_eq!(ln.text, "refactor the parser");
    }

    #[test]
    fn an_injected_user_turn_reads_as_system() {
        let ln = line(&text("▷ User: <task-notification>agent done</task-notification>", None));
        assert_eq!(ln.kind, LineKind::System);
    }

    #[test]
    fn a_peer_relay_becomes_a_peer_line_with_its_sender() {
        let ln = line(&text(
            "▷ User: Another Claude session sent a message:\n<cross-session-message from=\"a-1\" from-name=\"lane-b\">rebase please</cross-session-message>",
            None,
        ));
        assert_eq!(ln.kind, LineKind::Peer);
        assert_eq!(ln.peer_from.as_deref(), Some("lane-b"));
        assert_eq!(ln.text, "rebase please");
    }

    #[test]
    fn a_keepalive_tick_collapses_to_a_marker() {
        let ln = line(&text("▷ User: [cctui keep-alive 2/6]", None));
        assert_eq!(ln.kind, LineKind::Marker);
    }

    #[test]
    fn a_system_marker_is_not_mistaken_for_assistant_prose() {
        let ln = line(&text("session hibernated", Some("system_marker")));
        assert_eq!(ln.kind, LineKind::Marker);
    }

    #[test]
    fn assistant_usage_becomes_the_turn_footer() {
        let AgentEvent::Text { content, ts, .. } = text("done", None) else { unreachable!() };
        let event = AgentEvent::Text {
            content,
            meta: false,
            kind: None,
            operation: None,
            ts,
            message_id: Some("m-1".to_owned()),
            usage: Some(TokenUsage {
                tokens_in: 12_400,
                tokens_out: 1_100,
                cost_usd: 0.4,
                ..TokenUsage::default()
            }),
            seq: Some(2),
            turn_id: None,
        };
        let ln = line(&event);
        assert_eq!(ln.kind, LineKind::Assistant);
        assert_eq!(ln.message_id.as_deref(), Some("m-1"));
        let footer = ln.footer.expect("a footer");
        assert_eq!(footer.tokens_in, Some(12_400));
        assert_eq!(footer.tokens_out, Some(1_100));
    }

    #[test]
    fn a_turn_duration_annotation_becomes_a_footer_and_the_rest_vanish() {
        let ln = line(&text("turn_duration:38000", Some("turn_annotation")));
        assert_eq!(ln.kind, LineKind::Summary);
        assert_eq!(ln.footer.expect("a footer").duration_ms, Some(38_000));

        let file_history = text("file_history:src/a.rs", Some("turn_annotation"));
        assert!(agent_event_to_line(&file_history).is_none());
        assert!(agent_event_to_line(&text("turn_duration:0", Some("turn_annotation"))).is_none());
    }

    #[test]
    fn a_tool_call_carries_its_name_and_category() {
        let event = AgentEvent::ToolCall {
            tool: "Read".to_owned(),
            input: json!({"file_path": "src/parser.rs"}),
            kind: None,
            ts: 1,
            seq: Some(3),
        };
        let ln = line(&event);
        assert_eq!(ln.kind, LineKind::Tool { category: ToolCategory::Read });
        assert_eq!(ln.tool.as_deref(), Some("Read"));
        assert!(ln.text.contains("src/parser.rs"));
        assert!(ln.tool_input.is_none(), "only Edit/Write keep their input for a diff");
    }

    #[test]
    fn an_edit_keeps_its_input_so_the_view_can_diff_it() {
        let event = AgentEvent::ToolCall {
            tool: "Edit".to_owned(),
            input: json!({"file_path": "a.rs", "old_string": "a", "new_string": "b"}),
            kind: None,
            ts: 1,
            seq: Some(4),
        };
        let ln = line(&event);
        assert_eq!(ln.kind, LineKind::Tool { category: ToolCategory::Write });
        assert!(ln.tool_input.is_some());
    }

    #[test]
    fn a_provider_executed_tool_gets_the_server_category() {
        let event = AgentEvent::ToolCall {
            tool: "web_search".to_owned(),
            input: json!({}),
            kind: Some("server_tool_use".to_owned()),
            ts: 1,
            seq: Some(5),
        };
        assert_eq!(line(&event).kind, LineKind::Tool { category: ToolCategory::Server });
    }

    #[test]
    fn a_failed_result_carries_the_error_flag_and_its_tool() {
        let event = AgentEvent::ToolResult {
            tool: "Bash".to_owned(),
            output_summary: "exit 101 · 3 failed".to_owned(),
            kind: None,
            error: true,
            ts: 1,
            seq: Some(6),
        };
        let ln = line(&event);
        assert_eq!(ln.kind, LineKind::Result { error: true });
        assert_eq!(ln.tool.as_deref(), Some("Bash"));
        assert_eq!(ln.text, "exit 101 · 3 failed");
        assert!(ln.collapsible());
    }

    #[test]
    fn reset_and_compact_are_distinct_kinds() {
        assert_eq!(
            line(&AgentEvent::ContextReset { ts: 1, seq: None }).kind,
            LineKind::Reset
        );
        let compact =
            AgentEvent::CompactSummary { content: "we did X".to_owned(), ts: 1, seq: None };
        let ln = line(&compact);
        assert_eq!(ln.kind, LineKind::Compact);
        assert_eq!(ln.text, "we did X");
        assert!(
            agent_event_to_line(&AgentEvent::CompactSummary {
                content: " ".to_owned(),
                ts: 1,
                seq: None,
            })
            .is_none()
        );
    }

    #[test]
    fn a_turn_summary_falls_back_to_its_status_category() {
        let event = AgentEvent::TurnSummary {
            detail: "  ".to_owned(),
            status_category: Some("waiting_on_you".to_owned()),
            needs_action: true,
            ts: 1,
            seq: None,
        };
        let ln = line(&event);
        assert_eq!(ln.kind, LineKind::Summary);
        assert_eq!(ln.text, "waiting_on_you");
        assert!(ln.footer.expect("a footer").needs_action);
    }

    #[test]
    fn an_empty_reply_is_dropped() {
        let event = AgentEvent::Reply { content: " ".to_owned(), ts: 1, seq: None, turn_id: None };
        assert!(agent_event_to_line(&event).is_none());
    }
}
