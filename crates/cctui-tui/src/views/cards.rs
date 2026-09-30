//! In-session cards: the boxes that stack at the tail of a session's
//! transcript when the agent is blocked on a decision.
//!
//! [`card_lines`] is the one place cards are collected, so a new kind of
//! prompt (`AskUserQuestion`, plan approval) becomes one more call here.

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use serde_json::Value;

use crate::app::{App, PendingPermission};
use crate::theme;

/// Room for `│ ` and ` │`.
const FRAME: usize = 4;
const MIN_INNER: usize = 20;

/// Every card the session is currently showing, oldest first, ready to be
/// appended to the rendered transcript.
pub fn card_lines(app: &App, session_id: &str, width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for req in app.permissions.for_session(session_id) {
        lines.push(Line::from(""));
        lines.extend(permission_card(req, width));
    }
    lines
}

pub fn permission_card(req: &PendingPermission, width: u16) -> Vec<Line<'static>> {
    let inner = (width as usize).saturating_sub(FRAME).max(MIN_INNER);
    let mut lines = vec![top(&format!("⚠ Permission · {}", req.tool_name), inner)];

    let body = preview_lines(&req.tool_name, &req.input_preview, inner);
    if body.is_empty() {
        lines.push(row(vec![Span::styled(clip(&req.description, inner), theme::dim())], inner));
    } else {
        for line in body {
            lines.push(row(line.spans, inner));
        }
    }

    lines.push(bottom(inner));
    lines
}

/// `input_preview` is the tool input as JSON, truncated server-side: read the
/// interesting field out of it when it still parses, else show it raw.
fn preview_lines(tool: &str, preview: &str, inner: usize) -> Vec<Line<'static>> {
    let Ok(input) = serde_json::from_str::<Value>(preview) else {
        return text_lines(preview, inner, theme::bold());
    };
    match tool {
        "Bash" => {
            let command = string_field(&input, "command").unwrap_or_default();
            text_lines(&command, inner.saturating_sub(2), theme::bold())
                .into_iter()
                .map(|line| prefix(line, "$ "))
                .collect()
        }
        "Edit" | "Write" => diff_lines(tool, &input, inner)
            .unwrap_or_else(|| text_lines(&summary(tool, &input), inner, theme::bold())),
        _ => text_lines(&summary(tool, &input), inner, theme::bold()),
    }
}

fn diff_lines(tool: &str, input: &Value, inner: usize) -> Option<Vec<Line<'static>>> {
    let path = string_field(input, "file_path")?;
    let lines = match tool {
        "Edit" => super::conversation::edit_diff(input, &path, inner),
        _ => super::conversation::write_diff(input, &path, inner),
    }?;
    let mut out = vec![Line::from(Span::styled(clip(&path, inner), theme::bold()))];
    out.extend(lines);
    Some(out)
}

fn summary(tool: &str, input: &Value) -> String {
    let text = super::sessions::format_tool_input(tool, input);
    if text.is_empty() { input.to_string() } else { text }
}

fn string_field(input: &Value, key: &str) -> Option<String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Capped so one runaway command cannot push the composer off the screen.
const MAX_BODY_LINES: usize = 12;

fn text_lines(text: &str, inner: usize, style: Style) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = text
        .lines()
        .take(MAX_BODY_LINES)
        .map(|line| Line::from(Span::styled(clip(line, inner), style)))
        .collect();
    if text.lines().nth(MAX_BODY_LINES).is_some() {
        out.push(Line::from(Span::styled("…", theme::dim())));
    }
    out
}

fn prefix(mut line: Line<'static>, marker: &'static str) -> Line<'static> {
    line.spans.insert(0, Span::styled(marker, theme::dim()));
    line
}

fn top(title: &str, inner: usize) -> Line<'static> {
    let title = clip(title, inner);
    let fill = inner.saturating_sub(display_width(&title)) + 1;
    Line::from(vec![
        Span::styled("┌ ", theme::border_focused()),
        Span::styled(title, theme::bold()),
        Span::styled(format!("{}┐", "─".repeat(fill)), theme::border_focused()),
    ])
}

fn bottom(inner: usize) -> Line<'static> {
    let mut spans = vec![Span::styled("└ ", theme::border_focused())];
    let mut used = 0;
    for (key, label) in [("y", "allow"), ("n", "deny"), ("A", "allow + auto-approve")] {
        spans.push(Span::styled(format!("{key} "), theme::hotkey()));
        spans.push(Span::styled(format!("{label}   "), theme::hotkey_desc()));
        used += key.len() + 1 + label.len() + 3;
    }
    let fill = inner.saturating_sub(used) + 1;
    spans.push(Span::styled(format!("{}┘", "─".repeat(fill)), theme::border_focused()));
    Line::from(spans)
}

/// The body is framed, so it is truncated rather than wrapped: a rewrap would
/// slide the right-hand border off the card.
fn row(content: Vec<Span<'static>>, inner: usize) -> Line<'static> {
    let used: usize = content.iter().map(|s| display_width(&s.content)).sum();
    let mut spans = vec![Span::styled("│ ", theme::border_focused())];
    spans.extend(content);
    spans.push(Span::raw(" ".repeat(inner.saturating_sub(used))));
    spans.push(Span::styled(" │", theme::border_focused()));
    Line::from(spans)
}

fn display_width(text: &str) -> usize {
    text.chars().count()
}

fn clip(text: &str, max: usize) -> String {
    let text = text.trim_end();
    if display_width(text) <= max {
        return text.to_owned();
    }
    let head: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::{card_lines, permission_card, preview_lines};
    use crate::app::{App, PendingPermission};

    fn request(tool: &str, preview: &str) -> PendingPermission {
        PendingPermission {
            session_id: "s-a".to_owned(),
            request_id: "r1".to_owned(),
            tool_name: tool.to_owned(),
            description: "a tool wants to run".to_owned(),
            input_preview: preview.to_owned(),
        }
    }

    fn text(lines: &[ratatui::text::Line<'static>]) -> String {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn a_bash_request_shows_the_command_not_the_json() {
        let lines = permission_card(&request("Bash", r#"{"command":"rm -rf target/"}"#), 60);
        let rendered = text(&lines);
        assert!(rendered.contains("⚠ Permission · Bash"), "{rendered}");
        assert!(rendered.contains("$ rm -rf target/"), "{rendered}");
        assert!(!rendered.contains("\"command\""), "{rendered}");
        assert!(rendered.contains("y allow"), "{rendered}");
        assert!(rendered.contains("A allow + auto-approve"), "{rendered}");
    }

    #[test]
    fn an_edit_request_shows_a_diff() {
        let preview = serde_json::json!({
            "file_path": "src/main.rs",
            "old_string": "let a = 1;",
            "new_string": "let a = 2;",
        })
        .to_string();
        let rendered = text(&permission_card(&request("Edit", &preview), 80));
        assert!(rendered.contains("src/main.rs"), "{rendered}");
        assert!(rendered.contains("let a = 2;"), "{rendered}");
    }

    /// The preview is capped at 500 chars server-side, so it can arrive as
    /// broken JSON; the raw text is still better than nothing.
    #[test]
    fn a_truncated_preview_falls_back_to_raw_text() {
        let lines = preview_lines("Bash", "{\"command\":\"echo hel", 40);
        assert_eq!(text(&lines), "{\"command\":\"echo hel");
    }

    #[test]
    fn a_request_without_a_readable_input_shows_the_description() {
        let rendered = text(&permission_card(&request("Frobnicate", "null"), 60));
        let readable = rendered.contains("null") || rendered.contains("a tool wants to run");
        assert!(readable, "{rendered}");
    }

    #[test]
    fn a_long_body_is_clipped_to_the_card_width() {
        let long = "x".repeat(400);
        let preview = serde_json::json!({ "command": long }).to_string();
        for line in permission_card(&request("Bash", &preview), 50) {
            let width: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
            assert!(width <= 50, "a card line overflowed: {width}");
        }
    }

    #[test]
    fn cards_stack_only_for_their_own_session() {
        let mut app = App::new();
        let mut other = request("Bash", r#"{"command":"ls"}"#);
        other.session_id = "s-b".to_owned();
        app.permissions.push(request("Bash", r#"{"command":"ls"}"#));
        app.permissions.push(other);
        assert!(!card_lines(&app, "s-a", 60).is_empty());
        assert_eq!(card_lines(&app, "s-c", 60).len(), 0);
    }
}
