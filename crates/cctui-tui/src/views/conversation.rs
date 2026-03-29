use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use tui_markdown::from_str as markdown_from_str;

use crate::app::{App, ConversationLine, LineKind};
use crate::theme;

#[allow(clippy::cast_possible_truncation)]
pub fn draw(frame: &mut Frame, app: &App) {
    let Some(session) = app.selected_session() else {
        frame.render_widget(Paragraph::new("No session selected"), frame.area());
        return;
    };

    // Session info
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

    // Dynamic input height: grows with content, clamped to [3, 40% of screen]
    let input_lines = app.message_input.lines().count().max(1) + 2;
    let max_input = (frame.area().height as usize * 40 / 100).max(3);
    let input_height = input_lines.clamp(3, max_input) as u16;

    let [header_area, content_area, input_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(input_height),
    ])
    .areas(frame.area());

    // Header line with scroll position
    let lines = app.stream_buffer.get(&session.id);
    let header_text = lines.map_or_else(
        || {
            if branch.is_empty() {
                format!(" {project} on {machine} ── {model} ── {cost}")
            } else {
                format!(" {project} ({branch}) on {machine} ── {model} ── {cost}")
            }
        },
        |lines| {
            let visible_height = content_area.height as usize;
            let total = lines.iter().flat_map(|line| render_line_with_ts(line, app.show_timestamps)).count();
            let offset = if app.scroll_offset >= total.saturating_sub(visible_height) {
                total.saturating_sub(visible_height)
            } else {
                app.scroll_offset
            };
            let current_line = offset + 1;

            if branch.is_empty() {
                format!(" {project} on {machine} ── {model} ── {cost} [{current_line}/{total}]")
            } else {
                format!(
                    " {project} ({branch}) on {machine} ── {model} ── {cost} [{current_line}/{total}]"
                )
            }
        },
    );

    let header_line = Line::from(vec![Span::styled(header_text, theme::HEADER_BG)]);
    frame.render_widget(Paragraph::new(header_line), header_area);

    // Conversation content (full-width, no block)
    if let Some(conv_lines) = lines {
        let visible_height = content_area.height as usize;

        let all_display_lines: Vec<Line> = conv_lines
            .iter()
            .flat_map(|line| render_line_with_ts(line, app.show_timestamps))
            .collect();

        let total = all_display_lines.len();

        let offset = if app.scroll_offset >= total.saturating_sub(visible_height) {
            total.saturating_sub(visible_height)
        } else {
            app.scroll_offset
        };

        let display_lines: Vec<Line> =
            all_display_lines.iter().skip(offset).take(visible_height).cloned().collect();

        frame.render_widget(Paragraph::new(display_lines).wrap(Wrap { trim: false }), content_area);
    } else {
        frame.render_widget(
            Paragraph::new(Span::styled("No conversation data", theme::DIM)),
            content_area,
        );
    }

    // Input area with top border
    let input_block = if app.input_active {
        Block::default()
            .borders(Borders::TOP)
            .border_style(theme::BORDER_FOCUSED)
            .title(" Message (Enter to send, Shift+Enter for newline, Esc to cancel) ")
    } else {
        Block::default()
            .borders(Borders::TOP)
            .border_style(theme::BORDER_DIM)
            .title(" Start typing to compose message ")
    };

    let input_text = if app.message_input.is_empty() && !app.input_active {
        Paragraph::new("").block(input_block)
    } else {
        Paragraph::new(app.message_input.as_str()).block(input_block).wrap(Wrap { trim: false })
    };

    frame.render_widget(input_text, input_area);

    // Cursor position in input when active
    if app.input_active {
        let input_rect = input_area.inner(ratatui::layout::Margin { horizontal: 1, vertical: 1 });
        let cursor_x = (app.message_input.len() % (input_rect.width as usize))
            .min(input_rect.width.saturating_sub(1) as usize) as u16;
        let cursor_y = (app.message_input.lines().count().saturating_sub(1)) as u16;
        frame.set_cursor_position(Position::new(input_rect.x + cursor_x, input_rect.y + cursor_y));
    }
}

#[allow(clippy::too_many_lines)]
fn render_line_with_ts(line: &ConversationLine, show_ts: bool) -> Vec<Line<'static>> {
    let ts = if show_ts { format_timestamp(line.timestamp) } else { String::new() };

    match line.kind {
        LineKind::User => {
            let mut spans = Vec::new();
            if show_ts {
                spans.push(Span::styled(ts, theme::TIMESTAMP));
                spans.push(Span::raw("  "));
            }
            spans.push(Span::styled("You:", theme::USER_MSG));
            spans.push(Span::raw(" "));
            spans.push(Span::styled(line.text.clone(), theme::USER_MSG));

            vec![Line::from(spans)]
        }
        LineKind::Assistant => {
            let markdown_text = markdown_from_str(&line.text);
            if markdown_text.lines.is_empty() {
                let mut spans = Vec::new();
                if show_ts {
                    spans.push(Span::styled(ts, theme::TIMESTAMP));
                    spans.push(Span::raw("  "));
                }
                spans.push(Span::styled(line.text.clone(), theme::ASSISTANT_MSG));
                return vec![Line::from(spans)];
            }

            let mut result = Vec::new();
            for (idx, markdown_line) in markdown_text.lines.iter().enumerate() {
                let mut spans = Vec::new();
                if idx == 0 && show_ts {
                    spans.push(Span::styled(ts.clone(), theme::TIMESTAMP));
                    spans.push(Span::raw("  "));
                } else if idx > 0 && show_ts {
                    spans.push(Span::raw("       "));
                }

                let owned_spans: Vec<Span<'static>> = markdown_line
                    .spans
                    .iter()
                    .map(|span| Span::styled(span.content.to_string(), span.style))
                    .collect();
                spans.extend(owned_spans);

                result.push(Line::from(spans));
            }
            result
        }
        LineKind::ToolCall => {
            let text = &line.text;
            let (tool_name, detail) = if text.starts_with('[') {
                text.find(']')
                    .map_or(("", text.as_str()), |end| (&text[1..end], text[end + 2..].trim()))
            } else {
                ("", text.as_str())
            };

            let badge_style = match tool_name {
                "Bash" => theme::TOOL_BADGE_BASH,
                "Read" | "Write" | "Edit" | "Glob" => theme::TOOL_BADGE_FILE,
                _ => theme::TOOL_BADGE_FG,
            };

            let detail_style = match tool_name {
                "Bash" => theme::TOOL_COMMAND,
                "Read" | "Write" | "Edit" | "Glob" => theme::TOOL_PATH,
                _ => theme::TOOL_CALL,
            };

            let mut spans = Vec::new();
            if show_ts {
                spans.push(Span::styled(ts, theme::TIMESTAMP));
                spans.push(Span::raw("  "));
            }
            spans.push(Span::styled(format!(" {tool_name} "), badge_style));
            spans.push(Span::raw(" "));
            spans.push(Span::styled(detail.to_string(), detail_style));

            vec![Line::from(spans)]
        }
        LineKind::ToolResult => {
            let result_text = line.text.strip_prefix("  → ").unwrap_or(&line.text);

            let mut spans = Vec::new();
            if show_ts {
                spans.push(Span::styled(ts, theme::TIMESTAMP));
                spans.push(Span::raw("  "));
            }
            spans.push(Span::styled("→ ", theme::TOOL_RESULT_ARROW));
            spans.push(Span::styled(result_text.to_string(), theme::TOOL_RESULT));

            vec![Line::from(spans)]
        }
        LineKind::System => {
            let mut spans = Vec::new();
            if show_ts {
                spans.push(Span::styled(ts, theme::TIMESTAMP));
                spans.push(Span::raw("  "));
            }
            spans.push(Span::styled(line.text.clone(), theme::DIM));

            vec![Line::from(spans)]
        }
        LineKind::Reply => {
            let mut spans = Vec::new();
            if show_ts {
                spans.push(Span::styled(ts, theme::TIMESTAMP));
                spans.push(Span::raw("  "));
            }
            spans.push(Span::styled("◁ Reply: ", theme::MACHINE_HEADER));
            spans.push(Span::styled(line.text.clone(), theme::ASSISTANT_MSG));

            vec![Line::from(spans)]
        }
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
