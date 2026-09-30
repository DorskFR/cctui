use cctui_proto::ws::AgentEvent;

use super::state::{ConversationLine, LineKind, LineStatus};

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

/// A queue record: the enqueued prompt is the human's own message waiting its
/// turn, and only a withdrawal is worth a line of its own.
fn queue_op_line(content: &str, operation: Option<&str>, ts: i64) -> ConversationLine {
    let body = content.trim();
    match operation.unwrap_or("queued") {
        "queued" => ConversationLine::new(LineKind::User, body, ts).with_status(LineStatus::Queued),
        "removed" | "cleared" => {
            ConversationLine::new(LineKind::System, body, ts).with_status(LineStatus::Removed)
        }
        _ => ConversationLine::new(LineKind::System, String::new(), ts),
    }
}

pub fn agent_event_to_line(event: &AgentEvent) -> ConversationLine {
    match event {
        AgentEvent::Text { content, meta, ts, kind: text_kind, operation, .. } => {
            if text_kind.as_deref() == Some("queue_op") {
                return queue_op_line(content, operation.as_deref(), *ts);
            }
            let marker = matches!(text_kind.as_deref(), Some("system_marker" | "turn_annotation"));
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
            ConversationLine::new(kind, text, *ts)
        }
        AgentEvent::ToolCall { tool, input, ts, .. } => {
            let detail = super::prompt::historical_tool_text(tool, input)
                .unwrap_or_else(|| crate::views::sessions::format_tool_input(tool, input));
            // Keep raw input for Edit/Write so we can generate diffs during render
            let keep_input = matches!(tool.as_str(), "Edit" | "Write");
            ConversationLine {
                tool_input: if keep_input { Some(input.clone()) } else { None },
                ..ConversationLine::new(LineKind::ToolCall, format!("[{tool}] {detail}"), *ts)
            }
        }
        AgentEvent::ToolResult { output_summary, ts, .. } => {
            ConversationLine::new(LineKind::ToolResult, format!("  → {output_summary}"), *ts)
        }
        AgentEvent::Heartbeat { ts, .. } | AgentEvent::TurnEnd { ts, .. } => {
            ConversationLine::new(LineKind::System, String::new(), *ts)
        }
        // /clear boundary within one session.
        AgentEvent::ContextReset { ts, .. } => {
            ConversationLine::new(LineKind::System, "⟳ context reset (/clear · /compact)", *ts)
        }
        // /compact summary (no rotation; carries the summary text).
        AgentEvent::CompactSummary { content, ts, .. } => {
            ConversationLine::new(LineKind::System, format!("⟳ context compacted\n{content}"), *ts)
        }
        AgentEvent::TurnSummary { detail, ts, .. } => {
            ConversationLine::new(LineKind::System, format!("· {detail}"), *ts)
        }
        AgentEvent::Reply { content, ts, .. } => {
            ConversationLine::new(LineKind::Reply, content.clone(), *ts)
        }
    }
}
