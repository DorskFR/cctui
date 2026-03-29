use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::app::{App, ConversationLine, LineKind};
use crate::theme;
use crate::ui::markdown_render;

#[allow(clippy::too_many_lines, clippy::cast_possible_truncation)]
pub fn draw(frame: &mut Frame, app: &App) {
    let Some(session) = app.selected_session() else {
        frame.render_widget(Paragraph::new("No session selected"), frame.area());
        return;
    };

    // Header info
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

    // Full-width layout: header, content, separator, input
    let main_area = frame.area();

    let input_lines = app.message_input.lines().len().max(1) + 2;
    let max_input = (main_area.height as usize / 2).max(3);
    let input_height = input_lines.clamp(3, max_input) as u16;

    let [header_area, content_area, separator_area, input_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(input_height),
    ])
    .areas(main_area);

    // Header
    let lines = app.stream_buffer.get(&session.id);
    let header_text = if branch.is_empty() {
        format!(" {project} on {machine} ── {model} ── {cost}")
    } else {
        format!(" {project} ({branch}) on {machine} ── {model} ── {cost}")
    };
    let header_line = Line::from(vec![Span::styled(header_text, theme::HEADER_BG)]);
    frame.render_widget(Paragraph::new(header_line), header_area);

    // Conversation content
    if let Some(lines) = lines {
        let visible_height = content_area.height as usize;
        let all_display_lines: Vec<Line> =
            lines.iter().flat_map(|l| render_line(l, app.show_timestamps)).collect();
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

    // Separator
    let separator = Line::from(vec![Span::styled(
        "─".repeat(separator_area.width as usize),
        theme::BORDER_FOCUSED,
    )]);
    frame.render_widget(Paragraph::new(separator), separator_area);

    // Input
    let input_block = if app.input_active {
        Block::default()
            .borders(Borders::TOP)
            .border_style(theme::BORDER_FOCUSED)
            .title(" Message (Enter to send, Esc to cancel) ")
    } else {
        Block::default()
            .borders(Borders::TOP)
            .border_style(theme::BORDER_DIM)
            .title(" Press i to type ")
    };

    let mut textarea_widget = app.message_input.clone();
    textarea_widget.set_block(input_block);
    frame.render_widget(&textarea_widget, input_area);
}

/// Render a conversation line into display lines.
/// Each message type gets a role label, proper spacing, and full content.
#[allow(clippy::too_many_lines, clippy::redundant_clone)]
fn render_line(line: &ConversationLine, show_timestamps: bool) -> Vec<Line<'static>> {
    let mut result = Vec::new();
    let ts_prefix = if show_timestamps {
        let ts = format_timestamp(line.timestamp);
        format!("{ts}  ")
    } else {
        String::new()
    };

    match line.kind {
        LineKind::User => {
            // Blank line before user message for spacing
            result.push(Line::from(""));

            // Role label
            result.push(Line::from(vec![
                Span::raw(ts_prefix.clone()),
                Span::styled(
                    "You",
                    ratatui::style::Style::default()
                        .fg(ratatui::style::Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
            ]));

            // Message content
            for text_line in line.text.lines() {
                result.push(Line::from(vec![
                    Span::raw(if show_timestamps { "       " } else { "  " }),
                    Span::styled(text_line.to_string(), theme::USER_MSG),
                ]));
            }
        }
        LineKind::Assistant => {
            // Blank line before assistant message
            result.push(Line::from(""));

            // Role label
            result.push(Line::from(vec![
                Span::raw(ts_prefix.clone()),
                Span::styled(
                    "Assistant",
                    ratatui::style::Style::default()
                        .fg(ratatui::style::Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
            ]));

            // Render markdown content
            let md_text = markdown_render::render_markdown_text(&line.text);
            let indent = if show_timestamps { "       " } else { "  " };
            if md_text.lines.is_empty() {
                for text_line in line.text.lines() {
                    result.push(Line::from(vec![
                        Span::raw(indent),
                        Span::styled(text_line.to_string(), theme::ASSISTANT_MSG),
                    ]));
                }
            } else {
                for md_line in &md_text.lines {
                    let mut spans = vec![Span::raw(indent)];
                    for span in &md_line.spans {
                        spans.push(Span::styled(span.content.to_string(), span.style));
                    }
                    result.push(Line::from(spans));
                }
            }
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

            // Tool call header with badge — full text, no truncation
            result.push(Line::from(vec![
                Span::raw(ts_prefix.clone()),
                Span::styled(format!(" {tool_name} "), badge_style),
                Span::raw(" "),
                Span::styled(detail.to_string(), theme::TOOL_CALL),
            ]));
        }
        LineKind::ToolResult => {
            let result_text = line.text.strip_prefix("  → ").unwrap_or(&line.text);

            // Show full result with line breaks
            let indent = if show_timestamps { "       " } else { "  " };
            let lines_iter: Vec<&str> = result_text.lines().collect();

            if lines_iter.is_empty() {
                result.push(Line::from(vec![
                    Span::raw(ts_prefix.clone()),
                    Span::styled("→ ", theme::TOOL_RESULT_ARROW),
                    Span::styled("(empty)", theme::DIM),
                ]));
            } else {
                // First line with arrow
                result.push(Line::from(vec![
                    Span::raw(ts_prefix.clone()),
                    Span::styled("→ ", theme::TOOL_RESULT_ARROW),
                    Span::styled(lines_iter[0].to_string(), theme::TOOL_RESULT),
                ]));
                // Remaining lines indented
                for rest in &lines_iter[1..] {
                    result.push(Line::from(vec![
                        Span::raw(indent),
                        Span::styled(format!("  {rest}"), theme::TOOL_RESULT),
                    ]));
                }
            }
        }
        LineKind::System => {
            // Skip system messages (usually empty heartbeats)
            if !line.text.is_empty() {
                result.push(Line::from(vec![
                    Span::raw(ts_prefix),
                    Span::styled(line.text.clone(), theme::DIM),
                ]));
            }
        }
        LineKind::Reply => {
            result.push(Line::from(""));
            result.push(Line::from(vec![
                Span::raw(ts_prefix),
                Span::styled("◁ Reply ", theme::MACHINE_HEADER),
                Span::styled(line.text.clone(), theme::ASSISTANT_MSG),
            ]));
        }
    }

    result
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
