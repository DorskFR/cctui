use cctui_proto::api::SessionListItem;
use cctui_proto::classifier::Bucket;
use cctui_proto::ws::AgentEvent;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};

use crate::app::App;
use crate::theme;

pub fn draw(frame: &mut Frame, app: &App) {
    let [status_area, title_area, list_area, hotkeys_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    // Status bar
    draw_status_bar(frame, app, status_area);

    // Title line
    draw_title(frame, title_area);

    // Session list
    draw_session_list(frame, app, list_area);

    // Hotkeys
    crate::widgets::hotkeys::draw_session_hotkeys(frame, hotkeys_area);
}

fn draw_status_bar(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let total = app.sessions.len();
    let active = app.active_count;
    let line = Line::from(vec![
        Span::styled(" cctui ", theme::STATUS_BAR_BG),
        Span::raw(" "),
        Span::styled(format!("v{}", env!("CARGO_PKG_VERSION")), theme::DIM),
        Span::raw("  "),
        Span::styled(format!("{total} sessions"), theme::DIM),
        Span::raw("  "),
        Span::styled(format!("● {active} active"), theme::ACTIVE),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_title(frame: &mut Frame, area: ratatui::layout::Rect) {
    let line = Line::from(vec![Span::styled(" Sessions", theme::SECTION_TITLE)]);
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_session_list(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let flat = app.flattened_sessions();
    let mut items: Vec<ListItem> = Vec::new();

    // `flat` is already bucket-grouped (see App::flattened_sessions). Insert a
    // section header each time a top-level session opens a new bucket, and
    // remember where the selected session ends up once headers shift indices.
    let selected_flat = if app.selected_index < flat.len() { app.selected_index } else { 0 };
    let mut selected_render = 0usize;
    let mut current_bucket: Option<Bucket> = None;
    for (i, session) in flat.iter().enumerate() {
        // Only top-level sessions open a group; subagents stay under their parent.
        if session.parent_id.is_none() && current_bucket != Some(session.bucket) {
            current_bucket = Some(session.bucket);
            items.push(bucket_header(session.bucket));
        }
        if i == selected_flat {
            selected_render = items.len();
        }
        items.push(session_line(session));
    }

    // Show truncation hint if not showing all sessions
    if !app.show_all_sessions && app.sessions.len() > 5 {
        items.push(ListItem::new(Line::from(vec![Span::styled("   [a] show all", theme::DIM)])));
    }

    let list = List::new(items).highlight_style(theme::SELECTED).highlight_symbol("▸ ");

    let mut state = ListState::default();
    state.select(Some(selected_render));
    frame.render_stateful_widget(list, area, &mut state);
}

fn bucket_header(bucket: Bucket) -> ListItem<'static> {
    ListItem::new(Line::from(vec![Span::styled(
        format!(" {} ", bucket.label()),
        theme::SECTION_TITLE,
    )]))
}

fn session_line(s: &SessionListItem) -> ListItem<'static> {
    let icon = theme::status_icon(s.status);
    let icon_style = theme::status_style(s.status);

    let project = s
        .metadata
        .get("project_name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| basename(&s.working_dir));
    let branch = s.metadata.get("git_branch").and_then(serde_json::Value::as_str).unwrap_or("");
    let model = s.metadata.get("model").and_then(serde_json::Value::as_str).unwrap_or("");

    let uptime = format_uptime(s.uptime_secs);
    let cost = format!("${:.2}", s.token_usage.cost_usd);

    let adapter = s.adapter_id.as_ref().map_or("claude-code", |a| a.as_str());

    // Task-tool subagents (CCT-141) carry a parent id; indent them under the
    // parent with a tree marker instead of the leading whitespace.
    let is_subagent = s.parent_id.is_some();
    let mut spans = vec![
        Span::styled(if is_subagent { "    ↳ " } else { "   " }, theme::DIM),
        Span::styled(format!("{icon} "), icon_style),
        Span::styled(format!("[{adapter}] "), theme::DIM),
        Span::styled(project.to_string(), if is_subagent { theme::DIM } else { theme::BOLD }),
    ];

    if !branch.is_empty() {
        spans.push(Span::styled(format!(" ({branch})"), theme::BRANCH));
    }

    if !model.is_empty() {
        spans.push(Span::styled(format!("  {model}"), theme::MODEL));
    }

    spans.push(Span::styled(format!("  {uptime}"), theme::DIM));
    spans.push(Span::styled(format!("  {cost}"), theme::COST));

    ListItem::new(Line::from(spans))
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn format_uptime(secs: i64) -> String {
    if secs < 0 {
        return "?".to_string();
    }
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    }
}

// --- Event formatting (used by conversation view and main.rs) ---

pub fn format_tool_input(tool: &str, input: &serde_json::Value) -> String {
    let key = match tool {
        "Bash" => "command",
        "Read" | "Write" | "Edit" => "file_path",
        "Glob" | "Grep" => "pattern",
        "WebFetch" => "url",
        "WebSearch" => "query",
        "Agent" => "description",
        _ => "",
    };

    if !key.is_empty() {
        return input.get(key).and_then(serde_json::Value::as_str).unwrap_or("").to_string();
    }

    let s = serde_json::to_string(input).unwrap_or_default();
    if s.len() > 100 { format!("{}...", &s[..100]) } else { s }
}

pub fn agent_event_to_string(event: &AgentEvent) -> String {
    match event {
        AgentEvent::Text { content, .. } | AgentEvent::Reply { content, .. } => content.clone(),
        AgentEvent::ToolCall { tool, input, .. } => {
            let detail = format_tool_input(tool, input);
            format!("[{tool}] {detail}")
        }
        AgentEvent::ToolResult { output_summary, .. } => {
            format!("  → {output_summary}")
        }
        AgentEvent::Heartbeat { tokens_in, tokens_out, .. } => {
            format!("[heartbeat] in:{tokens_in} out:{tokens_out}")
        }
        AgentEvent::TurnEnd { .. } => String::new(),
        AgentEvent::ContextReset { .. } => "⟳ context reset (/clear · /compact)".to_owned(),
    }
}
