use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use tui_markdown::from_str as markdown_from_str;

use crate::app::{App, ConversationLine, LineKind};
use crate::theme;

/// Find the largest byte index <= `max_bytes` that is a char boundary.
fn truncate_at_char_boundary(s: &str, max_bytes: usize) -> &str {
    if max_bytes >= s.len() {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[allow(clippy::cast_possible_truncation)]
fn draw_sidebar(frame: &mut Frame, app: &App, area: Rect) {
    let flat = app.flattened_sessions();
    let lines: Vec<Line> = flat
        .iter()
        .take(9)
        .enumerate()
        .map(|(i, s)| {
            let num = format!("{} ", i + 1);
            let icon = theme::status_icon(&s.status);
            let project = s
                .metadata
                .get("project_name")
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| s.working_dir.rsplit('/').next().unwrap_or("?"));
            let truncated = truncate_at_char_boundary(project, 9);

            let style = if i == app.selected_index {
                theme::SELECTED
            } else {
                ratatui::style::Style::default()
            };
            Line::from(vec![
                Span::styled(num, theme::HOTKEY),
                Span::styled(format!("{icon} "), theme::status_style(&s.status)),
                Span::styled(truncated.to_string(), style),
            ])
        })
        .collect();

    frame.render_widget(Paragraph::new(lines), area);
}

#[allow(clippy::too_many_lines)]
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

    let main_area = if app.show_sidebar {
        let [sidebar_area, content_area] =
            Layout::horizontal([Constraint::Length(14), Constraint::Fill(1)]).areas(frame.area());
        draw_sidebar(frame, app, sidebar_area);
        content_area
    } else {
        frame.area()
    };

    // Dynamic input height: grows with content, clamped to [3, half screen]
    let input_lines = app.message_input.lines().len().max(1) + 2; // +2 for border + padding
    let max_input = (main_area.height as usize / 2).max(3);
    let input_height = input_lines.clamp(3, max_input) as u16;

    let [header_area, content_area, separator_area, input_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(input_height),
    ])
    .areas(main_area);

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
            let total = lines.iter().flat_map(render_line).count();
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

    // Conversation content (no block, direct render)
    if let Some(lines) = lines {
        let visible_height = content_area.height as usize;

        // Pre-compute all display lines (each ConversationLine may expand to multiple display lines)
        let all_display_lines: Vec<Line> = lines.iter().flat_map(render_line).collect();

        let total = all_display_lines.len();

        // Auto-scroll to bottom if offset is at or past the end
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

    // Separator line
    let separator = Line::from(vec![Span::styled(
        "─".repeat(separator_area.width as usize),
        theme::BORDER_FOCUSED,
    )]);
    frame.render_widget(Paragraph::new(separator), separator_area);

    // Input bar with top border only
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

#[allow(clippy::too_many_lines)]
fn render_line(line: &ConversationLine) -> Vec<Line<'static>> {
    let ts = format_timestamp(line.timestamp);

    match line.kind {
        LineKind::User => {
            let single_line = Line::from(vec![
                Span::styled(ts, theme::TIMESTAMP),
                Span::raw("  "),
                Span::styled("▷ ", theme::USER_MSG),
                Span::styled(line.text.clone(), theme::USER_MSG),
            ]);
            vec![single_line]
        }
        LineKind::Assistant => {
            let markdown_text = markdown_from_str(&line.text);
            if markdown_text.lines.is_empty() {
                let single_line = Line::from(vec![
                    Span::styled(ts, theme::TIMESTAMP),
                    Span::raw("  "),
                    Span::styled(line.text.clone(), theme::ASSISTANT_MSG),
                ]);
                return vec![single_line];
            }

            let mut result = Vec::new();
            for (idx, markdown_line) in markdown_text.lines.iter().enumerate() {
                let mut spans = Vec::new();
                if idx == 0 {
                    spans.push(Span::styled(ts.clone(), theme::TIMESTAMP));
                    spans.push(Span::raw("  "));
                } else {
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

            let truncated = if detail.len() > 80 {
                format!("{}…", truncate_at_char_boundary(detail, 80))
            } else {
                detail.to_string()
            };

            let single_line = Line::from(vec![
                Span::styled(ts, theme::TIMESTAMP),
                Span::raw("  "),
                Span::styled(format!(" {tool_name} "), badge_style),
                Span::raw(" "),
                Span::styled(truncated, detail_style),
            ]);
            vec![single_line]
        }
        LineKind::ToolResult => {
            let result_text = line.text.strip_prefix("  → ").unwrap_or(&line.text);
            let single_line = Line::from(vec![
                Span::styled(ts, theme::TIMESTAMP),
                Span::raw("  "),
                Span::styled("→", theme::TOOL_RESULT_ARROW),
                Span::raw(" "),
                Span::styled(result_text.to_string(), theme::TOOL_RESULT),
            ]);
            vec![single_line]
        }
        LineKind::System => {
            let single_line = Line::from(vec![
                Span::styled(ts, theme::TIMESTAMP),
                Span::raw("  "),
                Span::styled(line.text.clone(), theme::DIM),
            ]);
            vec![single_line]
        }
        LineKind::Reply => {
            let single_line = Line::from(vec![
                Span::styled(ts, theme::TIMESTAMP),
                Span::raw("  "),
                Span::styled("◁ Reply: ", theme::MACHINE_HEADER),
                Span::styled(line.text.clone(), theme::ASSISTANT_MSG),
            ]);
            vec![single_line]
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
