use cctui_proto::api::SessionListItem;
use cctui_proto::ws::AgentEvent;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};

use crate::app::App;
use crate::theme;

pub fn draw(frame: &mut Frame, app: &App) {
    let [status_area, list_area, hotkeys_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1), Constraint::Length(1)])
            .areas(frame.area());

    // Status bar
    draw_status_bar(frame, app, status_area);

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

    for (i, session) in flat.iter().enumerate() {
        // Machine group header
        if let Some(machine) = app.machine_header_at(i) {
            items.push(ListItem::new(Line::from(vec![
                Span::raw(" "),
                Span::styled(machine, theme::MACHINE_HEADER),
            ])));
        }

        items.push(session_line(session));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::BORDER_FOCUSED)
        .title(" Sessions ");

    // Map selected_index to the actual list index (accounting for headers)
    let list_index = compute_list_index(app);

    let list =
        List::new(items).block(block).highlight_style(theme::SELECTED).highlight_symbol("▸ ");

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
    let branch = s.metadata.get("git_branch").and_then(serde_json::Value::as_str).unwrap_or("");
    let model = s.metadata.get("model").and_then(serde_json::Value::as_str).unwrap_or("");
    let is_child = s.parent_id.is_some();

    let indent = if is_child { "   └─ " } else { "   " };
    let uptime = format_uptime(s.uptime_secs);
    let cost = format!("${:.2}", s.token_usage.cost_usd);

    let mut spans = vec![
        Span::raw(indent.to_string()),
        Span::styled(format!("{icon} "), icon_style),
        Span::styled(project.to_string(), theme::BOLD),
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

/// Map the app's `selected_index` to the list index that includes machine headers.
fn compute_list_index(app: &App) -> usize {
    let flat = app.flattened_sessions();
    let mut list_idx = 0;
    for (i, _) in flat.iter().enumerate() {
        if app.machine_header_at(i).is_some() {
            list_idx += 1; // header row
        }
        if i == app.selected_index {
            return list_idx;
        }
        list_idx += 1;
    }
    list_idx
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
        AgentEvent::Text { content, .. } => content.clone(),
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
