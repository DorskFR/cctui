use cctui_proto::ws::AgentEvent;

use super::state::{ConversationLine, LineKind};

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

pub(crate) fn agent_event_to_line(event: &AgentEvent) -> ConversationLine {
    match event {
        AgentEvent::Text { content, meta, ts, kind: text_kind, .. } => {
            let marker = matches!(
                text_kind.as_deref(),
                Some("system_marker" | "turn_annotation" | "queue_op")
            );
            let (kind, text) = if marker {
                (LineKind::System, content.clone())
            } else if content.starts_with("▷ User:") {
                let user_text = content.trim_start_matches("▷ User: ");
                // `meta` (set authoritatively at the adapter layer) marks a
                // system/agent-directed message — render it as System, not a
                // user line. Fall back to the local text-cleaning heuristic.
                if *meta {
                    (LineKind::System, user_text.to_owned())
                } else {
                    clean_user_message(user_text).map_or_else(
                        || (LineKind::System, String::new()),
                        |cleaned| (LineKind::User, cleaned),
                    )
                }
            } else {
                (LineKind::Assistant, content.clone())
            };
            ConversationLine { timestamp: *ts, kind, text, tool_input: None }
        }
        AgentEvent::ToolCall { tool, input, ts, .. } => {
            let detail = crate::views::sessions::format_tool_input(tool, input);
            // Keep raw input for Edit/Write so we can generate diffs during render
            let keep_input = matches!(tool.as_str(), "Edit" | "Write");
            ConversationLine {
                timestamp: *ts,
                kind: LineKind::ToolCall,
                text: format!("[{tool}] {detail}"),
                tool_input: if keep_input { Some(input.clone()) } else { None },
            }
        }
        AgentEvent::ToolResult { output_summary, ts, .. } => ConversationLine {
            timestamp: *ts,
            kind: LineKind::ToolResult,
            text: format!("  → {output_summary}"),
            tool_input: None,
        },
        AgentEvent::Heartbeat { ts, .. } | AgentEvent::TurnEnd { ts, .. } => ConversationLine {
            timestamp: *ts,
            kind: LineKind::System,
            text: String::new(),
            tool_input: None,
        },
        // /clear boundary within one session.
        AgentEvent::ContextReset { ts, .. } => ConversationLine {
            timestamp: *ts,
            kind: LineKind::System,
            text: "⟳ context reset (/clear · /compact)".to_owned(),
            tool_input: None,
        },
        // /compact summary (no rotation; carries the summary text).
        AgentEvent::CompactSummary { content, ts, .. } => ConversationLine {
            timestamp: *ts,
            kind: LineKind::System,
            text: format!("⟳ context compacted\n{content}"),
            tool_input: None,
        },
        AgentEvent::TurnSummary { detail, ts, .. } => ConversationLine {
            timestamp: *ts,
            kind: LineKind::System,
            text: format!("· {detail}"),
            tool_input: None,
        },
        AgentEvent::Reply { content, ts, .. } => ConversationLine {
            timestamp: *ts,
            kind: LineKind::Reply,
            text: content.clone(),
            tool_input: None,
        },
    }
}
