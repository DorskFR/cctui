//! Markdown and HTML transcript export, ported from the webui's `export.ts`.
//!
//! Byte-for-byte with `conversationToMarkdown`: the fixture in
//! `fixtures/parity/exportMarkdown.json` is replayed here and by the webui's
//! own test, so neither side can drift.
//!
//! Exports read [`AgentEvent`]s rather than the store's rendered lines: a tool
//! call's raw input becomes a fenced diff or shell block, and the store only
//! keeps that input for `Edit`/`Write`.

use cctui_proto::api::SessionListItem;
use cctui_proto::ws::AgentEvent;

use super::transcript::{self, USER_PREFIX};
use super::transcript_filter::{Category, Filter};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Markdown,
    Html,
}

impl Format {
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "md" | "markdown" => Some(Self::Markdown),
            "html" => Some(Self::Html),
            _ => None,
        }
    }

    pub const fn extension(self) -> &'static str {
        match self {
            Self::Markdown => "md",
            Self::Html => "html",
        }
    }
}

/// Session facts the header needs, so the renderers stay free of the wire type.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Meta {
    pub id: String,
    pub name: Option<String>,
    pub machine_name: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub working_dir: Option<String>,
}

impl Meta {
    #[must_use]
    pub fn of(session: &SessionListItem) -> Self {
        let text = |s: &Option<String>| s.as_ref().filter(|v| !v.is_empty()).cloned();
        Self {
            id: session.id.clone(),
            name: text(&session.name),
            machine_name: text(&session.machine_name),
            model: text(&session.model),
            effort: text(&session.effort),
            working_dir: Some(session.working_dir.clone()).filter(|d| !d.is_empty()),
        }
    }

    /// `name`, else the working dir, else the id — the webui's title order.
    #[must_use]
    pub fn title(&self) -> &str {
        self.name
            .as_deref()
            .or(self.working_dir.as_deref())
            .filter(|t| !t.is_empty())
            .unwrap_or(&self.id)
    }

    /// `cctui-<slug>-<date>.<ext>`, matching the webui's download name.
    #[must_use]
    pub fn file_stem(&self) -> String {
        let source = self.name.as_deref().unwrap_or(&self.id);
        let slug: String = source
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' })
            .collect();
        let slug = collapse_underscores(&slug);
        let slug: String = slug.chars().take(60).collect();
        if slug.is_empty() { "conversation".to_owned() } else { slug }
    }
}

/// `[^\w.-]+` collapses a run to a single `_` in the webui; do the same so the
/// two produce the same file name.
fn collapse_underscores(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c == '_' && out.ends_with('_') {
            continue;
        }
        out.push(c);
    }
    out
}

/// A run of injected turns that repeat verbatim is a monitor loop, not a human
/// typing twice: only a consecutive repeat with no `turn_id` demotes to `poll`.
#[derive(Debug, Default)]
pub struct PollSeen {
    last: Option<String>,
}

const POLL_PREFIXES: &[&str] = &["# Autonomous loop"];
const POLL_SENTINELS: &[&str] = &["<<autonomous-loop>>", "<<autonomous-loop-dynamic>>"];

#[must_use]
pub fn looks_poll(text: &str) -> bool {
    let trimmed = text.trim_start();
    POLL_PREFIXES.iter().any(|p| trimmed.starts_with(p))
        || POLL_SENTINELS.iter().any(|s| text.contains(s))
}

fn normalize_poll(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn poll_duplicate(content: &str, turn_id: Option<uuid::Uuid>, seen: &mut PollSeen) -> bool {
    let norm = normalize_poll(content);
    if norm.is_empty() {
        return false;
    }
    let dup = turn_id.is_none() && seen.last.as_deref() == Some(norm.as_str());
    seen.last = Some(norm);
    dup
}

fn breaks_poll_run(event: &AgentEvent) -> bool {
    match event {
        AgentEvent::Text { content, kind, .. } => {
            if matches!(kind.as_deref(), Some("turn_annotation" | "system_marker" | "queue_op")) {
                return false;
            }
            !content.starts_with(USER_PREFIX)
        }
        AgentEvent::ToolCall { .. }
        | AgentEvent::ToolResult { .. }
        | AgentEvent::ContextReset { .. }
        | AgentEvent::CompactSummary { .. } => true,
        _ => false,
    }
}

fn poll_role(
    content: &str,
    system: bool,
    seen: &mut PollSeen,
    turn_id: Option<uuid::Uuid>,
) -> Category {
    if looks_poll(content) {
        return Category::Poll;
    }
    if system {
        return Category::System;
    }
    if poll_duplicate(content, turn_id, seen) { Category::Poll } else { Category::User }
}

/// Fences a body, defusing an inner ` ``` ` exactly as the webui does.
fn fenced(body: &str, lang: &str) -> String {
    let safe = body.replace("```", "` ` `");
    format!("```{lang}\n{safe}\n```")
}

fn pretty_json(value: &serde_json::Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

fn string_field(input: &serde_json::Value, key: &str) -> Option<String> {
    input.get(key).and_then(serde_json::Value::as_str).map(str::to_owned)
}

fn tool_body(tool: &str, input: &serde_json::Value, pretty_diff: bool) -> String {
    if pretty_diff && input.get("old_string").is_some() && input.get("new_string").is_some() {
        let minus = lines_prefixed(&string_field(input, "old_string").unwrap_or_default(), "- ");
        let plus = lines_prefixed(&string_field(input, "new_string").unwrap_or_default(), "+ ");
        let path = string_field(input, "file_path").unwrap_or_default();
        let body = format!("{path}\n{minus}\n{plus}");
        return format!("**Tool · {tool}**\n\n{}", fenced(body.trim(), "diff"));
    }
    if let Some(command) = string_field(input, "command") {
        let desc = string_field(input, "description")
            .filter(|d| !d.trim().is_empty())
            .map_or_else(String::new, |d| format!("# {}\n", d.trim()));
        return format!("**Tool · {tool}**\n\n{}", fenced(&format!("{desc}{command}"), "sh"));
    }
    let json = pretty_json(input).replace("\\n", "\n").replace("\\t", "\t");
    format!("**Tool · {tool}**\n\n{}", fenced(&json, "json"))
}

fn lines_prefixed(text: &str, prefix: &str) -> String {
    text.split('\n').map(|l| format!("{prefix}{l}")).collect::<Vec<_>>().join("\n")
}

fn tool_category(tool: &str, kind: Option<&str>) -> Category {
    if kind == Some("server_tool_use") {
        Category::ServerTool
    } else if tool.starts_with("mcp__") {
        Category::Mcp
    } else {
        Category::Tool
    }
}

/// One event, at most one Markdown block. `None` is "nothing to export": an
/// empty payload, a filtered-out category, or an event with no prose at all.
#[allow(clippy::too_many_lines)]
fn markdown_block(
    event: &AgentEvent,
    filter: &Filter,
    seen: &mut PollSeen,
    pretty_diff: bool,
) -> Option<String> {
    if breaks_poll_run(event) {
        seen.last = None;
    }
    match event {
        AgentEvent::Text { content, meta, kind, operation, turn_id, .. } => {
            if content.trim().is_empty() {
                return None;
            }
            match kind.as_deref() {
                Some(k @ ("thinking" | "redacted_thinking")) => {
                    let category =
                        if k == "thinking" { Category::Thinking } else { Category::Redacted };
                    filter.shows(category).then(|| format!("**Thinking:**\n\n{content}"))
                }
                Some("turn_annotation") => None,
                Some("system_marker") => {
                    filter.shows(Category::Marker).then(|| format!("_{content}_"))
                }
                Some("queue_op") => {
                    if operation.as_deref().unwrap_or("queued") != "queued" {
                        return None;
                    }
                    filter.shows(Category::User).then(|| format!("**User:**\n\n{content}"))
                }
                _ => match content.strip_prefix(USER_PREFIX) {
                    Some(rest) => {
                        let body = rest.trim_start();
                        if let Some(peer) = transcript::parse_peer_message(body) {
                            if !filter.shows(Category::Peer) {
                                return None;
                            }
                            let who = peer
                                .from
                                .map_or_else(|| "Peer".to_owned(), |from| format!("Peer · {from}"));
                            return Some(format!("**{who}:**\n\n{}", peer.body));
                        }
                        let system = *meta || transcript::looks_meta(body);
                        let role = poll_role(body, system, seen, *turn_id);
                        if !filter.shows(role) {
                            return None;
                        }
                        let who = match role {
                            Category::Poll => "Poll",
                            Category::System => "System",
                            _ => "User",
                        };
                        Some(format!("**{who}:**\n\n{body}"))
                    }
                    // An attachment is assistant prose here: the TUI has no
                    // separate attachment category to filter on.
                    None => filter
                        .shows(Category::Assistant)
                        .then(|| format!("**Assistant:**\n\n{content}")),
                },
            }
        }
        AgentEvent::Reply { content, turn_id, .. } => {
            if content.trim().is_empty() {
                return None;
            }
            let role = poll_role(content, false, seen, *turn_id);
            if !filter.shows(role) {
                return None;
            }
            let who = if role == Category::Poll { "Poll" } else { "User" };
            Some(format!("**{who}:**\n\n{content}"))
        }
        AgentEvent::ToolCall { tool, input, kind, .. } => filter
            .shows(tool_category(tool, kind.as_deref()))
            .then(|| tool_body(tool, input, pretty_diff)),
        AgentEvent::ToolResult { tool, output_summary, error, .. } => {
            let category = if *error { Category::Error } else { Category::Result };
            filter
                .shows(category)
                .then(|| format!("**Result · {tool}**\n\n{}", fenced(output_summary, "")))
        }
        AgentEvent::ContextReset { .. } => filter
            .shows(Category::Reset)
            .then(|| "---\n\n_⟳ context reset · /clear or /compact_\n\n---".to_owned()),
        AgentEvent::CompactSummary { content, .. } => {
            if !filter.shows(Category::Compact) || content.trim().is_empty() {
                return None;
            }
            Some(format!("**Compacted context:**\n\n{content}"))
        }
        AgentEvent::TurnSummary { detail, status_category, needs_action, .. } => {
            if !filter.shows(Category::Summary) {
                return None;
            }
            let detail = if detail.trim().is_empty() {
                status_category.as_deref().unwrap_or_default().trim()
            } else {
                detail.trim()
            };
            if detail.is_empty() {
                return None;
            }
            let label = if *needs_action { "Needs action" } else { "Summary" };
            Some(format!("_{label}: {detail}_"))
        }
        AgentEvent::Heartbeat { .. } | AgentEvent::TurnEnd { .. } => None,
    }
}

#[must_use]
pub fn to_markdown(meta: &Meta, events: &[AgentEvent], filter: &Filter) -> String {
    let mut head = vec![format!("# {}", meta.title()), String::new()];
    let mut bits = vec![format!("session: `{}`", meta.id)];
    if let Some(machine) = &meta.machine_name {
        bits.push(format!("machine: {machine}"));
    }
    if let Some(model) = &meta.model {
        let effort = meta.effort.as_ref().map_or_else(String::new, |e| format!(" · {e}"));
        bits.push(format!("model: {model}{effort}"));
    }
    if let Some(cwd) = &meta.working_dir {
        bits.push(format!("cwd: `{cwd}`"));
    }
    head.push(bits.join(" · "));
    head.push(String::new());

    let mut seen = PollSeen::default();
    let body: Vec<String> =
        events.iter().filter_map(|e| markdown_block(e, filter, &mut seen, true)).collect();
    format!("{}{}\n", head.join("\n"), body.join("\n\n"))
}

/// A self-contained page: the same blocks as the Markdown export, escaped, with
/// the stylesheet inlined so the file opens anywhere.
#[must_use]
pub fn to_html(meta: &Meta, events: &[AgentEvent], filter: &Filter) -> String {
    let mut seen = PollSeen::default();
    let blocks: Vec<String> = events
        .iter()
        .filter_map(|e| markdown_block(e, filter, &mut seen, true))
        .map(|block| format!("<div class=\"msg\"><pre>{}</pre></div>", escape_html(&block)))
        .collect();
    let title = escape_html(meta.title());
    let mut bits = vec![format!("<span><b>session</b> {}</span>", escape_html(&meta.id))];
    if let Some(machine) = &meta.machine_name {
        bits.push(format!("<span><b>machine</b> {}</span>", escape_html(machine)));
    }
    if let Some(model) = &meta.model {
        let effort =
            meta.effort.as_ref().map_or_else(String::new, |e| format!(" · {}", escape_html(e)));
        bits.push(format!("<span><b>model</b> {}{effort}</span>", escape_html(model)));
    }
    if let Some(cwd) = &meta.working_dir {
        bits.push(format!("<span><b>cwd</b> {}</span>", escape_html(cwd)));
    }
    bits.push(format!("<span><b>events</b> {}</span>", blocks.len()));
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\" />\n<meta \
         name=\"viewport\" content=\"width=device-width, initial-scale=1\" />\n<title>{title} — \
         cctui transcript</title>\n<style>{CSS}</style>\n</head>\n<body>\n<div \
         class=\"page\">\n<header><h1>{title}</h1><div class=\"meta\">{}</div></header>\n{}\n<footer>Exported \
         from cctui</footer>\n</div>\n</body>\n</html>\n",
        bits.join(""),
        blocks.join("\n")
    )
}

const CSS: &str = "\
:root{color-scheme:dark}\
body{background:#14161a;color:#d7dae0;font:13px/1.55 ui-monospace,SFMono-Regular,Menlo,monospace;margin:0}\
.page{max-width:900px;margin:0 auto;padding:24px}\
header h1{font-size:18px;margin:0 0 6px}\
.meta{display:flex;flex-wrap:wrap;gap:10px;color:#8b919c;font-size:11px;margin-bottom:18px}\
.msg{border-left:2px solid #2a2e36;padding:2px 0 2px 10px;margin:10px 0}\
pre{margin:0;white-space:pre-wrap;word-break:break-word}\
footer{margin-top:28px;color:#8b919c;font-size:11px;text-align:center}";

fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use cctui_proto::ws::AgentEvent;
    use serde_json::json;

    use super::{Format, Meta, escape_html, looks_poll, to_html, to_markdown};
    use crate::app::transcript_filter::{Category, Filter};

    fn meta() -> Meta {
        Meta {
            id: "s-1".to_owned(),
            name: Some("cctui".to_owned()),
            machine_name: Some("orion".to_owned()),
            model: Some("opus".to_owned()),
            effort: None,
            working_dir: Some("/home/dev/cctui".to_owned()),
        }
    }

    fn text(content: &str, kind: Option<&str>) -> AgentEvent {
        AgentEvent::Text {
            content: content.to_owned(),
            meta: false,
            kind: kind.map(str::to_owned),
            operation: None,
            ts: 1,
            message_id: None,
            usage: None,
            seq: Some(1),
            turn_id: None,
        }
    }

    #[test]
    fn the_header_carries_the_session_facts() {
        let md = to_markdown(&meta(), &[], &Filter::default());
        assert_eq!(
            md,
            "# cctui\n\nsession: `s-1` · machine: orion · model: opus · cwd: `/home/dev/cctui`\n\n"
        );
    }

    #[test]
    fn the_title_falls_back_to_the_working_dir_then_the_id() {
        let mut m = meta();
        m.name = None;
        assert_eq!(m.title(), "/home/dev/cctui");
        m.working_dir = None;
        assert_eq!(m.title(), "s-1");
    }

    #[test]
    fn a_file_stem_is_slugified_and_capped() {
        let mut m = meta();
        m.name = Some("my session/with weird:chars".to_owned());
        assert_eq!(m.file_stem(), "my_session_with_weird_chars");
        m.name = Some("!!!".to_owned());
        assert_eq!(m.file_stem(), "_", "a run collapses to one underscore");
        m.name = Some("x".repeat(80));
        assert_eq!(m.file_stem().len(), 60);
    }

    #[test]
    fn roles_get_the_labels_the_webui_uses() {
        let events = vec![
            text("▷ User: ship it", None),
            text("on it", None),
            text("weighing options", Some("thinking")),
            text("hibernated", Some("system_marker")),
        ];
        let mut filter = Filter::default();
        filter.show_all();
        let md = to_markdown(&meta(), &events, &filter);
        assert!(md.contains("**User:**\n\nship it"), "{md}");
        assert!(md.contains("**Assistant:**\n\non it"));
        assert!(md.contains("**Thinking:**\n\nweighing options"));
        assert!(md.contains("_hibernated_"));
    }

    #[test]
    fn an_injected_turn_is_a_system_block_and_a_loop_wake_is_a_poll() {
        let events = vec![
            text("▷ User: <system-reminder>be good</system-reminder>", None),
            text("▷ User: # Autonomous loop\ncheck the queue", None),
        ];
        let mut filter = Filter::default();
        filter.show_all();
        let md = to_markdown(&meta(), &events, &filter);
        assert!(md.contains("**System:**"), "{md}");
        assert!(md.contains("**Poll:**"), "{md}");
        assert!(looks_poll("  # Autonomous loop"));
        assert!(looks_poll("wake: <<autonomous-loop>>"));
        assert!(!looks_poll("a normal turn"));
    }

    #[test]
    fn a_repeated_injected_turn_without_a_turn_id_demotes_to_a_poll() {
        let events = vec![text("▷ User: keep going", None), text("▷ User: keep going", None)];
        let mut filter = Filter::default();
        filter.show_all();
        let md = to_markdown(&meta(), &events, &filter);
        assert_eq!(md.matches("**User:**").count(), 1);
        assert_eq!(md.matches("**Poll:**").count(), 1, "the consecutive repeat demotes: {md}");
    }

    #[test]
    fn an_edit_becomes_a_diff_fence_and_a_bash_call_a_shell_fence() {
        let edit = AgentEvent::ToolCall {
            tool: "Edit".to_owned(),
            input: json!({"file_path": "a.rs", "new_string": "b", "old_string": "a"}),
            kind: None,
            ts: 1,
            seq: None,
        };
        let bash = AgentEvent::ToolCall {
            tool: "Bash".to_owned(),
            input: json!({"command": "cargo test", "description": "run the suite"}),
            kind: None,
            ts: 1,
            seq: None,
        };
        let md = to_markdown(&meta(), &[edit, bash], &Filter::default());
        assert!(md.contains("**Tool · Edit**\n\n```diff\na.rs\n- a\n+ b\n```"), "{md}");
        assert!(md.contains("**Tool · Bash**\n\n```sh\n# run the suite\ncargo test\n```"), "{md}");
    }

    #[test]
    fn any_other_tool_input_becomes_a_json_fence_with_real_newlines() {
        let call = AgentEvent::ToolCall {
            tool: "Grep".to_owned(),
            input: json!({"pattern": "a\nb"}),
            kind: None,
            ts: 1,
            seq: None,
        };
        let md = to_markdown(&meta(), &[call], &Filter::default());
        assert!(md.contains("```json\n{\n  \"pattern\": \"a\nb\"\n}\n```"), "{md}");
    }

    #[test]
    fn a_fence_inside_a_body_cannot_break_out() {
        let result = AgentEvent::ToolResult {
            tool: "Read".to_owned(),
            output_summary: "```rust\nfn x() {}\n```".to_owned(),
            kind: None,
            error: false,
            ts: 1,
            seq: None,
        };
        let md = to_markdown(&meta(), &[result], &Filter::default());
        assert!(md.contains("` ` `rust"), "{md}");
        assert_eq!(md.matches("```").count(), 2, "only the wrapping fence survives");
    }

    #[test]
    fn a_failed_result_follows_the_error_category() {
        let failed = AgentEvent::ToolResult {
            tool: "Bash".to_owned(),
            output_summary: "exit 1".to_owned(),
            kind: None,
            error: true,
            ts: 1,
            seq: None,
        };
        let mut filter = Filter::default();
        assert!(to_markdown(&meta(), std::slice::from_ref(&failed), &filter).contains("exit 1"));
        filter.toggle(Category::Error);
        assert!(!to_markdown(&meta(), &[failed], &filter).contains("exit 1"));
    }

    #[test]
    fn the_filter_decides_what_reaches_the_file() {
        let events = vec![text("▷ User: ship it", None), text("on it", None)];
        let mut filter = Filter::default();
        filter.cycle();
        let md = to_markdown(&meta(), &events, &filter);
        assert!(md.contains("**Assistant:**"));
        assert!(!md.contains("**User:**"), "the assistant quick filter drops the user turn");
    }

    #[test]
    fn heartbeats_and_empty_payloads_export_nothing() {
        let events = vec![
            AgentEvent::Heartbeat { tokens_in: 1, tokens_out: 1, cost_usd: 0.0, ts: 1, seq: None },
            AgentEvent::TurnEnd { ts: 1, seq: None },
            text("   ", None),
            AgentEvent::CompactSummary { content: " ".to_owned(), ts: 1, seq: None },
        ];
        let md = to_markdown(&meta(), &events, &Filter::default());
        assert_eq!(md, to_markdown(&meta(), &[], &Filter::default()));
    }

    #[test]
    fn a_context_reset_is_a_ruled_off_marker() {
        let md = to_markdown(
            &meta(),
            &[AgentEvent::ContextReset { ts: 1, seq: None }],
            &Filter::default(),
        );
        assert!(md.contains("---\n\n_⟳ context reset · /clear or /compact_\n\n---"), "{md}");
    }

    #[test]
    fn html_escapes_the_blocks_and_stands_alone() {
        let events = vec![text("▷ User: <script>alert(1)</script>", None)];
        let mut filter = Filter::default();
        filter.show_all();
        let html = to_html(&meta(), &events, &filter);
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("&lt;script&gt;"), "{html}");
        assert!(!html.contains("<script>"));
        assert!(html.contains("<style>"), "the stylesheet is inlined");
        assert!(html.contains("<b>events</b> 1"));
        assert_eq!(escape_html("a&b\"c'"), "a&amp;b&quot;c&#39;");
    }

    #[test]
    fn a_format_is_parsed_from_the_command_word() {
        assert_eq!(Format::parse("md"), Some(Format::Markdown));
        assert_eq!(Format::parse("markdown"), Some(Format::Markdown));
        assert_eq!(Format::parse("html"), Some(Format::Html));
        assert_eq!(Format::parse("pdf"), None);
        assert_eq!(Format::Markdown.extension(), "md");
    }
}

/// Replays `fixtures/parity/exportMarkdown.json`, the same file the webui's
/// `export.test.ts` replays against `conversationToMarkdown`.
#[cfg(test)]
mod parity {
    use cctui_proto::ws::AgentEvent;
    use serde_json::Value;

    use super::{Meta, to_markdown};
    use crate::app::transcript_filter::Filter;

    fn fixture() -> Value {
        let path =
            concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/parity/exportMarkdown.json");
        let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {path}: {e}"))
    }

    #[test]
    fn the_markdown_export_matches_the_webui_byte_for_byte() {
        let fx = fixture();
        let session = &fx["session"];
        let text = |key: &str| session[key].as_str().map(str::to_owned);
        let meta = Meta {
            id: text("id").unwrap_or_default(),
            name: text("name"),
            machine_name: text("machine_name"),
            model: text("model"),
            effort: text("effort"),
            working_dir: text("working_dir"),
        };
        let events: Vec<AgentEvent> = fx["events"]
            .as_array()
            .expect("an event list")
            .iter()
            .map(|e| {
                serde_json::from_value(e.clone())
                    .unwrap_or_else(|err| panic!("undecodable fixture event {e}: {err}"))
            })
            .collect();

        let mut filter = Filter::default();
        filter.show_all();
        let expected = fx["expected"].as_str().expect("an expected string");
        assert_eq!(to_markdown(&meta, &events, &filter), expected);
    }
}
