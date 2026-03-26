use cctui_proto::ws::AgentEvent;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, ListState, Paragraph, Wrap};
use uuid::Uuid;

use crate::app::{App, DetailMode, Pane};
use crate::widgets::hotkeys::HotkeysBar;
use crate::widgets::status_bar::StatusBar;
use crate::widgets::tree::SessionTree;

pub fn draw(frame: &mut Frame, app: &App) {
    let [status_area, main_area, hotkeys_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1), Constraint::Length(1)])
            .areas(frame.area());

    // Status bar
    frame.render_widget(
        StatusBar::new(
            app.sessions.len(),
            app.active_count,
            app.aggregate_tokens,
            app.aggregate_cost,
        ),
        status_area,
    );

    // Hotkeys bar
    frame.render_widget(HotkeysBar, hotkeys_area);

    // Main area: 30% tree / 70% detail
    let [tree_area, detail_area] =
        Layout::horizontal([Constraint::Percentage(30), Constraint::Fill(1)]).areas(main_area);

    draw_tree(frame, app, tree_area);
    draw_detail(frame, app, detail_area);

    // Message input overlay
    if let Some(ref input) = app.message_input {
        draw_message_input(frame, input, main_area);
    }
}

fn draw_tree(frame: &mut Frame, app: &App, area: Rect) {
    let is_active = app.active_pane == Pane::Tree;
    let border_style = if is_active {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let block =
        Block::default().title(" Sessions ").borders(Borders::ALL).border_style(border_style);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let flat = app.flattened_sessions();
    let tree = SessionTree::new(&flat);
    let mut list_state = ListState::default();
    list_state.select(Some(app.selected_index));

    frame.render_stateful_widget(tree, inner, &mut list_state);
}

fn draw_detail(frame: &mut Frame, app: &App, area: Rect) {
    let is_active = app.active_pane == Pane::Detail;
    let border_style = if is_active {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let title = match app.detail_mode {
        DetailMode::Conversation => " Conversation ",
        DetailMode::Log => " Log ",
    };

    let block = Block::default().title(title).borders(Borders::ALL).border_style(border_style);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    match app.selected_session() {
        None => {
            let text =
                Paragraph::new("No session selected").style(Style::default().fg(Color::DarkGray));
            frame.render_widget(text, inner);
        }
        Some(session) => {
            draw_session_detail(frame, app, session.id, inner);
        }
    }
}

fn draw_session_detail(frame: &mut Frame, app: &App, session_id: Uuid, area: Rect) {
    let [header_area, content_area] =
        Layout::vertical([Constraint::Length(2), Constraint::Fill(1)]).areas(area);

    // Header: session id + status hint
    if let Some(session) = app.sessions.iter().find(|s| s.id == session_id) {
        let header = Line::from(vec![
            Span::styled(
                session.id.to_string(),
                Style::default().fg(Color::Gray).add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(session.working_dir.as_str(), Style::default().fg(Color::DarkGray)),
        ]);
        frame.render_widget(Paragraph::new(header), header_area);
    }

    // Stream content
    let lines: Vec<Line> = app.stream_buffer.get(&session_id).map_or_else(
        || vec![Line::from(Span::styled("No stream data", Style::default().fg(Color::DarkGray)))],
        |entries| entries.iter().map(|s| Line::from(s.as_str())).collect(),
    );

    let content = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(content, content_area);
}

fn draw_message_input(frame: &mut Frame, input: &str, parent_area: Rect) {
    let width = parent_area.width.saturating_sub(4).min(80);
    let x = parent_area.x + (parent_area.width.saturating_sub(width)) / 2;
    let y = parent_area.y + parent_area.height.saturating_sub(4);
    let overlay_area = Rect::new(x, y, width, 3);

    frame.render_widget(Clear, overlay_area);

    let block = Block::default()
        .title(" Message ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));

    let inner = block.inner(overlay_area);
    frame.render_widget(block, overlay_area);

    let text = format!("{input}_");
    frame.render_widget(Paragraph::new(text.as_str()), inner);
}

fn format_agent_event(event: &AgentEvent) -> String {
    match event {
        AgentEvent::Text { content, .. } => content.clone(),
        AgentEvent::ToolCall { tool, .. } => format!("[tool:{tool}]"),
        AgentEvent::ToolResult { tool, output_summary, .. } => {
            format!("[{tool}] {output_summary}")
        }
        AgentEvent::Heartbeat { tokens_in, tokens_out, .. } => {
            format!("[heartbeat] in:{tokens_in} out:{tokens_out}")
        }
    }
}

// Used in Task 20 for populating stream_buffer
pub fn agent_event_to_string(event: &AgentEvent) -> String {
    format_agent_event(event)
}
