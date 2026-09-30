//! Server-side markdown rendering of a normalized transcript, the Rust
//! counterpart of `conversationToMarkdown` (`webui/src/lib/export.ts`).
//!
//! Input is what [`crate::normalize::for_client`] already produced, so roles are
//! decided by [`crate::normalize::client_category`] and this module never
//! re-derives them. `CctuiHistory` is the first consumer; a topic's predecessor
//! brief is meant to be the second, which is why the byte budget and the role
//! filter are parameters rather than constants.

use serde_json::Value;

/// A transcript's identity line.
#[derive(Debug, Clone, Default)]
pub struct Header {
    pub session_id: String,
    pub name: Option<String>,
    pub adapter: Option<String>,
    pub machine: Option<String>,
    pub state: &'static str,
}

/// A rendered page, with the cursors a caller needs to ask for the next one.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Rendered {
    pub text: String,
    pub events: usize,
    /// `seq` of the oldest event in the page; pass it as `before` to page back.
    pub first_seq: Option<i64>,
    /// `seq` of the newest event in the page; pass it as `after` for a delta.
    pub last_seq: Option<i64>,
    /// The byte budget cut the page short, so older events are missing.
    pub truncated: bool,
}

/// The prefix `normalize` stores a human turn under.
const USER_PREFIX: &str = "▷ User:";

/// A single tool argument blob is not a transcript; summarise it.
const TOOL_SNIPPET: usize = 400;

fn text_of(payload: &Value) -> Option<String> {
    let s = |k: &str| payload.get(k).and_then(Value::as_str);
    match s("type").unwrap_or_default() {
        "text" => {
            let content = s("content").unwrap_or_default();
            Some(content.strip_prefix(USER_PREFIX).unwrap_or(content).trim().to_owned())
        }
        "reply" => s("text").or_else(|| s("content")).map(str::to_owned),
        "tool_call" => {
            let input = payload.get("input").map_or_else(String::new, |v| {
                v.as_str().map_or_else(|| v.to_string(), str::to_owned)
            });
            Some(snippet(input.trim(), TOOL_SNIPPET))
        }
        "tool_result" => {
            let out = payload.get("output_summary").map_or_else(String::new, |v| {
                v.as_str().map_or_else(|| v.to_string(), str::to_owned)
            });
            Some(snippet(out.trim(), TOOL_SNIPPET))
        }
        "turn_summary" | "compact_summary" => s("content").or_else(|| s("text")).map(str::to_owned),
        _ => None,
    }
}

/// Truncate on a char boundary, marking the cut.
#[must_use]
pub fn snippet(raw: &str, max: usize) -> String {
    if raw.chars().count() <= max {
        return raw.to_owned();
    }
    let kept: String = raw.chars().take(max).collect();
    format!("{kept}…")
}

/// One event as a markdown block, or `None` when it renders to nothing.
#[must_use]
pub fn block(payload: &Value) -> Option<(&'static str, String)> {
    let (role, tool) = crate::normalize::client_category(payload);
    if role.is_empty() {
        return None;
    }
    let body = text_of(payload)?;
    if body.is_empty() {
        return None;
    }
    let rendered = tool.map_or_else(
        || format!("**{role}**\n\n{body}"),
        |tool| format!("**{role}: {tool}**\n\n{body}"),
    );
    Some((role, rendered))
}

/// Render `events` — `(seq, normalized payload)`, oldest first — keeping only
/// `roles` when it is non-empty, within `budget` bytes of body text.
///
/// The budget is spent from the NEWEST end: a truncated page keeps the tail of
/// the conversation, which is what a reader asking "what happened here" wants.
#[must_use]
pub fn render(
    header: &Header,
    events: &[(i64, Value)],
    roles: &[String],
    budget: usize,
) -> Rendered {
    let wanted = |role: &str| roles.is_empty() || roles.iter().any(|r| r == role);
    let mut blocks: Vec<(i64, String)> = Vec::new();
    let mut spent = 0usize;
    let mut truncated = false;
    for (seq, payload) in events.iter().rev() {
        let Some((role, text)) = block(payload) else { continue };
        if !wanted(role) {
            continue;
        }
        if spent + text.len() > budget && !blocks.is_empty() {
            truncated = true;
            break;
        }
        spent += text.len();
        blocks.push((*seq, text));
    }
    blocks.reverse();

    let mut out = String::with_capacity(spent + 256);
    out.push_str(&head(header));
    for (i, (_, text)) in blocks.iter().enumerate() {
        if i > 0 {
            out.push_str("\n\n");
        }
        out.push_str(text);
    }
    out.push('\n');
    Rendered {
        text: out,
        events: blocks.len(),
        first_seq: blocks.first().map(|(s, _)| *s),
        last_seq: blocks.last().map(|(s, _)| *s),
        truncated,
    }
}

fn head(h: &Header) -> String {
    let title = h.name.as_deref().map(str::trim).filter(|n| !n.is_empty()).unwrap_or(&h.session_id);
    let mut bits = vec![format!("session: `{}`", h.session_id), format!("state: {}", h.state)];
    if let Some(adapter) = h.adapter.as_deref() {
        bits.push(format!("adapter: {adapter}"));
    }
    if let Some(machine) = h.machine.as_deref() {
        bits.push(format!("machine: {machine}"));
    }
    format!("# {title}\n\n{}\n\n", bits.join(" · "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn header() -> Header {
        Header {
            session_id: "sess-1".into(),
            name: Some("lane a".into()),
            adapter: Some("codex".into()),
            machine: Some("box-b".into()),
            state: "archived",
        }
    }

    fn user(text: &str) -> Value {
        json!({ "type": "text", "content": format!("▷ User: {text}"), "meta": false })
    }

    fn assistant(text: &str) -> Value {
        json!({ "type": "text", "content": text, "role": "Assistant" })
    }

    #[test]
    fn a_transcript_renders_with_a_header_and_role_labelled_blocks() {
        let events = vec![(1, user("do the thing")), (2, assistant("done")), (3, user("thanks"))];
        let out = render(&header(), &events, &[], 64 * 1024);
        assert_eq!(out.events, 3);
        assert_eq!(out.first_seq, Some(1));
        assert_eq!(out.last_seq, Some(3));
        assert!(!out.truncated);
        assert!(out.text.starts_with("# lane a\n"), "{}", out.text);
        assert!(out.text.contains("session: `sess-1`"), "{}", out.text);
        assert!(out.text.contains("state: archived"), "{}", out.text);
        assert!(out.text.contains("adapter: codex"), "{}", out.text);
        assert!(out.text.contains("**user**\n\ndo the thing"), "{}", out.text);
        assert!(out.text.contains("**assistant**\n\ndone"), "{}", out.text);
        assert!(!out.text.contains("▷ User:"), "the storage prefix must not leak: {}", out.text);
    }

    #[test]
    fn a_nameless_session_is_titled_by_its_id() {
        let h = Header { name: None, ..header() };
        assert!(render(&h, &[], &[], 1024).text.starts_with("# sess-1\n"));
        let blank = Header { name: Some("   ".into()), ..header() };
        assert!(render(&blank, &[], &[], 1024).text.starts_with("# sess-1\n"));
    }

    #[test]
    fn a_role_filter_keeps_only_the_named_roles() {
        let events = vec![
            (1, user("ask")),
            (2, assistant("answer")),
            (3, json!({ "type": "tool_call", "tool": "Bash", "input": "git status" })),
        ];
        let only_user = render(&header(), &events, &["user".to_owned()], 64 * 1024);
        assert_eq!(only_user.events, 1);
        assert!(only_user.text.contains("ask"));
        assert!(!only_user.text.contains("answer"));

        let conversation =
            render(&header(), &events, &["user".to_owned(), "assistant".to_owned()], 64 * 1024);
        assert_eq!(conversation.events, 2);
        assert!(!conversation.text.contains("git status"));

        let tools = render(&header(), &events, &["tool".to_owned()], 64 * 1024);
        assert_eq!(tools.events, 1);
        assert!(tools.text.contains("**tool: Bash**\n\ngit status"), "{}", tools.text);
    }

    /// The budget is spent newest-first, so a truncated page is the tail of the
    /// conversation and `truncated` says so.
    #[test]
    fn the_budget_keeps_the_newest_events_and_reports_truncation() {
        let events: Vec<(i64, Value)> =
            (1..=20).map(|i| (i, assistant(&format!("line {i} {}", "x".repeat(50))))).collect();
        let out = render(&header(), &events, &[], 300);
        assert!(out.truncated);
        assert!(out.events < 20 && out.events > 0, "{}", out.events);
        assert_eq!(out.last_seq, Some(20), "the newest event must survive");
        assert!(out.text.contains("line 20"), "{}", out.text);
        assert!(!out.text.contains("line 1 "), "{}", out.text);
    }

    /// A single event larger than the whole budget is still returned: an empty
    /// page with `truncated` would be a silent, unpageable dead end.
    #[test]
    fn one_oversized_event_is_returned_rather_than_an_empty_page() {
        let events = vec![(1, assistant(&"y".repeat(5_000)))];
        let out = render(&header(), &events, &[], 100);
        assert_eq!(out.events, 1);
        assert!(!out.truncated);
    }

    #[test]
    fn events_that_render_to_nothing_are_dropped() {
        let events = vec![
            (1, json!({ "type": "text", "content": "▷ User:   " })),
            (2, json!({ "type": "turn_annotation" })),
            (3, json!({ "type": "text", "kind": "turn_annotation", "content": "x" })),
            (4, assistant("real")),
        ];
        let out = render(&header(), &events, &[], 64 * 1024);
        assert_eq!(out.events, 1);
        assert_eq!(out.first_seq, Some(4));
    }

    /// A peer message keeps its own role, so a session reading its parent's
    /// history can tell which turns came from another agent.
    #[test]
    fn a_peer_turn_renders_under_the_peer_role() {
        let raw =
            "▷ User: <cross-session-message from=\"x\">check the tests</cross-session-message>";
        let out = render(&header(), &[(1, json!({ "type": "text", "content": raw }))], &[], 4096);
        assert_eq!(out.events, 1);
        assert!(out.text.contains("**peer**"), "{}", out.text);
    }

    #[test]
    fn a_tool_input_is_summarised_not_dumped() {
        let big =
            json!({ "type": "tool_call", "tool": "Write", "input": { "body": "z".repeat(5_000) } });
        let out = render(&header(), &[(1, big)], &[], 64 * 1024);
        assert!(out.text.contains('…'), "{}", out.text);
        assert!(out.text.len() < 1_200, "{}", out.text.len());
    }

    #[test]
    fn snippets_cut_on_char_boundaries() {
        assert_eq!(snippet("héllo", 10), "héllo");
        assert_eq!(snippet("héllo", 3), "hél…");
        assert_eq!(snippet("", 3), "");
    }

    #[test]
    fn an_empty_transcript_still_renders_its_header() {
        let out = render(&header(), &[], &[], 1024);
        assert_eq!(out.events, 0);
        assert!(out.first_seq.is_none() && out.last_seq.is_none());
        assert!(!out.truncated);
        assert!(out.text.contains("# lane a"));
    }
}
