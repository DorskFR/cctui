use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Style};
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
    let header_text = if branch.is_empty() {
        format!(" {project} on {machine} ── {model} ── {cost}")
    } else {
        format!(" {project} ({branch}) on {machine} ── {model} ── {cost}")
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(header_text, theme::HEADER_BG))),
        header_area,
    );

    // Conversation
    if let Some(lines) = app.stream_buffer.get(&session.id) {
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
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "─".repeat(separator_area.width as usize),
            theme::BORDER_FOCUSED,
        ))),
        separator_area,
    );

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

/// Count how many display lines a conversation line produces (for scroll math).
pub fn count_display_lines(line: &ConversationLine, show_timestamps: bool) -> usize {
    render_line(line, show_timestamps).len()
}

// -- Styles: muted/subdued palette --

// Role labels: soft, not shouting
const LABEL_YOU: Style = Style::new().fg(Color::Rgb(130, 170, 200)); // soft blue
const LABEL_ASSISTANT: Style = Style::new().fg(Color::Rgb(180, 140, 100)); // warm muted orange

// Tool badges: dark background tints, light text — subtle not aggressive
const TOOL_READ: Style = Style::new().fg(Color::Rgb(140, 160, 180)).bg(Color::Rgb(30, 40, 55)); // slate
const TOOL_WRITE: Style = Style::new().fg(Color::Rgb(200, 180, 130)).bg(Color::Rgb(50, 45, 25)); // dark amber
const TOOL_MCP: Style = Style::new().fg(Color::Rgb(170, 140, 180)).bg(Color::Rgb(45, 30, 50)); // dark plum
const TOOL_DETAIL: Style = Style::new().fg(Color::Rgb(100, 100, 100)); // muted gray
const TOOL_RESULT_STYLE: Style = Style::new().fg(Color::Rgb(90, 90, 90)); // dimmer gray
const ARROW: Style = Style::new().fg(Color::Rgb(80, 80, 80));

fn tool_badge_style(tool_name: &str) -> (Style, &'static str) {
    match tool_name {
        // Read tools
        "Read" | "Glob" | "Grep" | "WebFetch" | "WebSearch" | "LSP" => (TOOL_READ, "read"),
        // Write tools
        "Write" | "Edit" | "Bash" | "NotebookEdit" => (TOOL_WRITE, "write"),
        // MCP tools (prefixed with mcp__)
        name if name.starts_with("mcp__") => (TOOL_MCP, "mcp"),
        // Everything else
        _ => (TOOL_READ, "tool"),
    }
}

#[allow(clippy::too_many_lines, clippy::redundant_clone)]
fn render_line(line: &ConversationLine, show_timestamps: bool) -> Vec<Line<'static>> {
    let mut result = Vec::new();
    let ts = if show_timestamps {
        format!("{} ", format_timestamp(line.timestamp))
    } else {
        String::new()
    };

    match line.kind {
        LineKind::User => {
            result.push(Line::from(""));
            result.push(Line::from(vec![Span::raw(ts), Span::styled("❯ You", LABEL_YOU)]));
            for text_line in line.text.lines() {
                result.push(Line::from(Span::styled(
                    text_line.to_string(),
                    Style::default().fg(Color::Rgb(210, 210, 210)),
                )));
            }
        }
        LineKind::Assistant => {
            result.push(Line::from(""));
            result.push(Line::from(vec![
                Span::raw(ts),
                Span::styled("● Assistant", LABEL_ASSISTANT),
            ]));
            let md_text = markdown_render::render_markdown_text(&line.text);
            if md_text.lines.is_empty() {
                for text_line in line.text.lines() {
                    result.push(Line::from(Span::raw(text_line.to_string())));
                }
            } else {
                for md_line in &md_text.lines {
                    let spans: Vec<Span<'static>> = md_line
                        .spans
                        .iter()
                        .map(|s| Span::styled(s.content.to_string(), s.style))
                        .collect();
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

            let (badge_style, _category) = tool_badge_style(tool_name);

            // Shorten MCP tool names: mcp__server__tool → server:tool
            let display_name = if tool_name.starts_with("mcp__") {
                tool_name.strip_prefix("mcp__").unwrap_or(tool_name).replacen("__", ":", 1)
            } else {
                tool_name.to_string()
            };

            result.push(Line::from(vec![
                Span::raw(ts),
                Span::styled(format!(" {display_name} "), badge_style),
                Span::raw(" "),
                Span::styled(detail.to_string(), TOOL_DETAIL),
            ]));
        }
        LineKind::ToolResult => {
            let result_text = line.text.strip_prefix("  → ").unwrap_or(&line.text);
            let lines_vec: Vec<&str> = result_text.lines().collect();
            if lines_vec.is_empty() {
                result.push(Line::from(vec![Span::raw(ts), Span::styled("→ (empty)", ARROW)]));
            } else {
                result.push(Line::from(vec![
                    Span::raw(ts),
                    Span::styled("→ ", ARROW),
                    Span::styled(lines_vec[0].to_string(), TOOL_RESULT_STYLE),
                ]));
                for rest in &lines_vec[1..] {
                    result.push(Line::from(Span::styled(format!("  {rest}"), TOOL_RESULT_STYLE)));
                }
            }
        }
        LineKind::System => {
            if !line.text.is_empty() {
                result.push(Line::from(Span::styled(line.text.clone(), theme::DIM)));
            }
        }
        LineKind::Reply => {
            result.push(Line::from(""));
            result.push(Line::from(vec![
                Span::raw(ts),
                Span::styled("◁ Reply ", LABEL_ASSISTANT),
                Span::raw(line.text.clone()),
            ]));
        }
    }

    result
}

fn format_timestamp(ts: i64) -> String {
    if ts == 0 {
        return "     ".to_string();
    }
    chrono::DateTime::from_timestamp(ts, 0).map_or_else(
        || "??:??".to_string(),
        |dt| dt.with_timezone(&chrono::Local).format("%H:%M").to_string(),
    )
}
