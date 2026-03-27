use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::app::{App, ConversationLine, LineKind};
use crate::theme;

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
            let truncated = if project.len() > 9 { &project[..9] } else { project };

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

    let [header_area, content_area, separator_area, input_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(2),
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
            let total = lines.len();
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
        let total = lines.len();

        // Auto-scroll to bottom if offset is at or past the end
        let offset = if app.scroll_offset >= total.saturating_sub(visible_height) {
            total.saturating_sub(visible_height)
        } else {
            app.scroll_offset
        };

        let display_lines: Vec<Line> =
            lines.iter().skip(offset).take(visible_height).map(render_line).collect();

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

    let input_text = if app.input_active {
        format!("> {}_", app.message_input)
    } else if app.message_input.is_empty() {
        String::new()
    } else {
        format!("> {}", app.message_input)
    };

    frame.render_widget(Paragraph::new(input_text).block(input_block), input_area);
}

fn render_inline_markdown(text: &str) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut remaining = text;

    while !remaining.is_empty() {
        if let Some(code_start) = remaining.find('`') {
            if code_start > 0 {
                spans.push(Span::styled(remaining[..code_start].to_string(), theme::ASSISTANT_MSG));
            }
            let after_backtick = &remaining[code_start + 1..];
            if let Some(code_end) = after_backtick.find('`') {
                let code = &after_backtick[..code_end];
                spans.push(Span::styled(format!("`{code}`"), theme::MD_CODE));
                remaining = &after_backtick[code_end + 1..];
            } else {
                spans.push(Span::styled(remaining.to_string(), theme::ASSISTANT_MSG));
                break;
            }
        } else if let Some(bold_start) = remaining.find("**") {
            if bold_start > 0 {
                spans.push(Span::styled(remaining[..bold_start].to_string(), theme::ASSISTANT_MSG));
            }
            let after_bold = &remaining[bold_start + 2..];
            if let Some(bold_end) = after_bold.find("**") {
                let bold_text = &after_bold[..bold_end];
                spans.push(Span::styled(bold_text.to_string(), theme::MD_BOLD));
                remaining = &after_bold[bold_end + 2..];
            } else {
                spans.push(Span::styled(remaining.to_string(), theme::ASSISTANT_MSG));
                break;
            }
        } else if let Some(italic_start) = remaining.find('*') {
            if italic_start > 0 {
                spans.push(Span::styled(
                    remaining[..italic_start].to_string(),
                    theme::ASSISTANT_MSG,
                ));
            }
            let after_italic = &remaining[italic_start + 1..];
            if let Some(italic_end) = after_italic.find('*') {
                let italic_text = &after_italic[..italic_end];
                spans.push(Span::styled(italic_text.to_string(), theme::MD_ITALIC));
                remaining = &after_italic[italic_end + 1..];
            } else {
                spans.push(Span::styled(remaining.to_string(), theme::ASSISTANT_MSG));
                break;
            }
        } else {
            spans.push(Span::styled(remaining.to_string(), theme::ASSISTANT_MSG));
            break;
        }
    }

    if spans.is_empty() {
        spans.push(Span::styled(text.to_string(), theme::ASSISTANT_MSG));
    }
    spans
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
        LineKind::Assistant => {
            let ts_span = Span::styled(ts, theme::TIMESTAMP);
            let rendered_spans = render_inline_markdown(&line.text);
            let mut all_spans = vec![ts_span, Span::raw("  ")];
            all_spans.extend(rendered_spans);
            Line::from(all_spans)
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
                format!("{}…", &detail[..80])
            } else {
                detail.to_string()
            };

            Line::from(vec![
                Span::styled(ts, theme::TIMESTAMP),
                Span::raw("  "),
                Span::styled(format!(" {tool_name} "), badge_style),
                Span::raw(" "),
                Span::styled(truncated, detail_style),
            ])
        }
        LineKind::ToolResult => {
            let result_text =
                if line.text.starts_with("  → ") { &line.text[4..] } else { &line.text };
            Line::from(vec![
                Span::styled(ts, theme::TIMESTAMP),
                Span::raw("  "),
                Span::styled("→", theme::TOOL_RESULT_ARROW),
                Span::raw(" "),
                Span::styled(result_text.to_string(), theme::TOOL_RESULT),
            ])
        }
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
