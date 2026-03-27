use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::app::{App, ConversationLine, LineKind};
use crate::theme;

pub fn draw(frame: &mut Frame, app: &App) {
    let Some(session) = app.selected_session() else {
        frame.render_widget(Paragraph::new("No session selected"), frame.area());
        return;
    };

    // Title bar info
    let project = session
        .metadata
        .get("project_name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let branch =
        session.metadata.get("git_branch").and_then(serde_json::Value::as_str).unwrap_or("");
    let model = session.metadata.get("model").and_then(serde_json::Value::as_str).unwrap_or("");
    let cost = format!("${:.2}", session.token_usage.cost_usd);
    let machine = &session.machine_id;

    let title = if branch.is_empty() {
        format!(" {project} on {machine} ── {model} ── {cost} ")
    } else {
        format!(" {project} ({branch}) on {machine} ── {model} ── {cost} ")
    };

    let [main_area, input_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(3)]).areas(frame.area());

    // Conversation content
    let block =
        Block::default().borders(Borders::ALL).border_style(theme::BORDER_FOCUSED).title(title);
    let inner = block.inner(main_area);
    frame.render_widget(block, main_area);

    let lines = app.stream_buffer.get(&session.id);
    if let Some(lines) = lines {
        let visible_height = inner.height as usize;
        let total = lines.len();

        // Auto-scroll to bottom if offset is at or past the end
        let offset = if app.scroll_offset >= total.saturating_sub(visible_height) {
            total.saturating_sub(visible_height)
        } else {
            app.scroll_offset
        };

        let display_lines: Vec<Line> =
            lines.iter().skip(offset).take(visible_height).map(render_line).collect();

        frame.render_widget(Paragraph::new(display_lines).wrap(Wrap { trim: false }), inner);
    } else {
        frame
            .render_widget(Paragraph::new(Span::styled("No conversation data", theme::DIM)), inner);
    }

    // Input bar
    let input_block = if app.input_active {
        Block::default()
            .borders(Borders::ALL)
            .border_style(theme::BORDER_FOCUSED)
            .title(" Message (Enter to send, Esc to cancel) ")
    } else {
        Block::default()
            .borders(Borders::ALL)
            .border_style(theme::BORDER_DIM)
            .title(" Press i to type ")
    };

    let input_text = if app.input_active {
        format!("> {}_", app.message_input)
    } else if app.message_input.is_empty() {
        String::new()
    } else {
        format!("> {}", app.message_input)
    };

    frame.render_widget(Paragraph::new(input_text).block(input_block), input_area);
}

fn render_line(line: &ConversationLine) -> Line<'static> {
    let ts = format_timestamp(line.timestamp);

    match line.kind {
        LineKind::User => Line::from(vec![
            Span::styled(ts, theme::TIMESTAMP),
            Span::raw("  "),
            Span::styled("▷ ", theme::USER_MSG),
            Span::styled(line.text.clone(), theme::USER_MSG),
        ]),
        LineKind::Assistant => Line::from(vec![
            Span::styled(ts, theme::TIMESTAMP),
            Span::raw("  "),
            Span::raw(line.text.clone()),
        ]),
        LineKind::ToolCall => Line::from(vec![
            Span::styled(ts, theme::TIMESTAMP),
            Span::raw("  "),
            Span::styled(line.text.clone(), theme::TOOL_CALL),
        ]),
        LineKind::ToolResult => Line::from(vec![
            Span::styled(ts, theme::TIMESTAMP),
            Span::raw("  "),
            Span::styled(line.text.clone(), theme::TOOL_RESULT),
        ]),
        LineKind::System => Line::from(vec![
            Span::styled(ts, theme::TIMESTAMP),
            Span::raw("  "),
            Span::styled(line.text.clone(), theme::DIM),
        ]),
    }
}

fn format_timestamp(ts: i64) -> String {
    if ts == 0 {
        return "     ".to_string();
    }
    let dt = chrono::DateTime::from_timestamp(ts, 0);
    dt.map_or_else(
        || "??:??".to_string(),
        |dt| {
            let local = dt.with_timezone(&chrono::Local);
            local.format("%H:%M").to_string()
        },
    )
}
