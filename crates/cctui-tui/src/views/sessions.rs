use cctui_proto::api::SessionListItem;
use cctui_proto::ws::AgentEvent;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};

use crate::app::App;
use crate::theme;

pub fn draw(frame: &mut Frame, app: &App) {
    let [status_area, list_area, hotkeys_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1), Constraint::Length(1)])
            .areas(frame.area());

    // Status bar
    draw_status_bar(frame, app, status_area);

    // Session list (full-width)
    draw_session_list(frame, app, list_area);

    // Hotkeys
    crate::widgets::hotkeys::draw_session_hotkeys(frame, hotkeys_area);
}

fn draw_status_bar(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let total = app.sessions.len();
    let active = app.active_count;
    let line = Line::from(vec![
        Span::styled(" cctui ", theme::STATUS_BAR_BG),
        Span::raw("  "),
        Span::styled(format!("{total} sessions"), theme::DIM),
        Span::raw("  "),
        Span::styled(format!("● {active} active"), theme::ACTIVE),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_session_list(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let flat = app.flattened_sessions();
    let mut items: Vec<ListItem> = Vec::new();

    for session in &flat {
        items.push(session_line(session));
    }

    // Show truncation hint if not showing all sessions
    if !app.show_all_sessions && app.sessions.len() > 5 {
        items.push(ListItem::new(Line::from(vec![Span::styled("   [a] show all", theme::DIM)])));
    }

    // Map selected_index to the actual list index
    let list_index = if app.selected_index < flat.len() { app.selected_index } else { 0 };

    let list = List::new(items).highlight_style(theme::SELECTED).highlight_symbol("▸ ");

    let mut state = ListState::default();
    state.select(Some(list_index));
    frame.render_stateful_widget(list, area, &mut state);
}

fn session_line(s: &SessionListItem) -> ListItem<'static> {
    let icon = theme::status_icon(&s.status);
    let icon_style = theme::status_style(&s.status);

    let project = s
        .metadata
        .get("project_name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| basename(&s.working_dir));
    let model = s.metadata.get("model").and_then(serde_json::Value::as_str).unwrap_or("");

    let uptime = format_uptime(s.uptime_secs);
    let cost = format!("${:.2}", s.token_usage.cost_usd);

    // Shortened model name (just the model identifier, no "claude-" prefix)
    let model_short = model.strip_prefix("claude-").unwrap_or(model);

    let mut spans = vec![
        Span::raw("   "),
        Span::styled(format!("{icon} "), icon_style),
        Span::styled(project.to_string(), theme::BOLD),
    ];

    if !model_short.is_empty() {
        spans.push(Span::styled(format!("  {model_short}"), theme::MODEL));
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
    }
}
