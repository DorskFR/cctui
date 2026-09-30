//! The right-hand panel: the agent's todo list, then its subagents.

use cctui_proto::api::SessionListItem;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::App;
use crate::app::sidebar::{Panel, panel, todo_mark, todo_text};
use crate::theme;

/// Whether the sidebar gets a column: it is opt-in, a narrow terminal keeps
/// the transcript instead, and any overlay drawn over the conversation —
/// diagnose, the pagers, the pickers — gets the full width to itself rather
/// than leaving a panel showing past its margins.
pub fn visible(app: &App, area_width: u16) -> bool {
    app.ui.sidebar_open
        && area_width >= crate::app::sidebar::MIN_TERMINAL_WIDTH
        && matches!(app.view(), crate::app::View::Conversation | crate::app::View::Sidebar)
}

pub fn draw(frame: &mut Frame, area: Rect, app: &App, session: &SessionListItem, focused: bool) {
    let border = if focused { theme::border_focused() } else { theme::border_dim() };
    let block = Block::default().borders(Borders::LEFT).border_style(border);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(Paragraph::new(lines(&panel(app, session), inner.width, focused)), inner);
}

/// Pure so the layout can be read in a test without a terminal.
fn lines(panel: &Panel, width: u16, focused: bool) -> Vec<Line<'static>> {
    let width = width as usize;
    let mut out = vec![heading(&todo_title(panel))];
    if panel.todos.is_empty() {
        out.push(faint("no todo list yet", width));
    }
    for todo in &panel.todos {
        let mark = todo_mark(&todo.status);
        let style = match todo.status.as_str() {
            "completed" => theme::dim(),
            "in_progress" => theme::active(),
            _ => theme::hotkey_desc(),
        };
        out.push(Line::from(vec![
            Span::styled(format!(" {mark} "), style),
            Span::styled(clip(todo_text(todo), width.saturating_sub(5)), style),
        ]));
    }

    out.push(Line::from(""));
    out.push(heading(&agents_title(panel)));
    if panel.children.is_empty() {
        out.push(faint("no subagents", width));
    }
    for (index, child) in panel.children.iter().enumerate() {
        let selected = focused && index == panel.cursor;
        let marker = if selected { "▸" } else { " " };
        let label_style = if child.running { theme::bold() } else { theme::dim() };
        let room = width.saturating_sub(child.state.len() + 4);
        out.push(Line::from(vec![
            Span::styled(format!(" {marker} "), theme::hotkey()),
            Span::styled(clip(&child.label, room), label_style),
            Span::styled(format!("  {}", child.state), theme::dim()),
        ]));
    }

    if let Some(parent) = &panel.parent {
        out.push(Line::from(""));
        out.push(Line::from(vec![
            Span::styled(" u ", theme::hotkey()),
            Span::styled("up to ", theme::hotkey_desc()),
            Span::styled(clip(parent, width.saturating_sub(8)), theme::dim()),
        ]));
    }
    out
}

fn todo_title(panel: &Panel) -> String {
    if panel.todos.is_empty() {
        return "Todo".to_owned();
    }
    format!("Todo {}/{}", panel.done, panel.todos.len())
}

fn agents_title(panel: &Panel) -> String {
    if panel.children.is_empty() {
        return "Subagents".to_owned();
    }
    format!("Subagents {}/{} live", panel.running, panel.children.len())
}

fn heading(text: &str) -> Line<'static> {
    Line::from(Span::styled(format!(" {text}"), theme::section_title()))
}

fn faint(text: &str, width: usize) -> Line<'static> {
    Line::from(Span::styled(format!(" {}", clip(text, width.saturating_sub(1))), theme::dim()))
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let head: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::{lines, visible};
    use crate::app::sidebar::MIN_TERMINAL_WIDTH;
    use crate::app::{App, sidebar};
    use crate::testsupport::{session, subagent, todo};

    fn app_with_children() -> App {
        let mut app = App::new();
        let mut parent = session("s-parent", "alpha", "active", "working");
        parent.todos =
            vec![todo("completed", "Read", None), todo("in_progress", "Wire", Some("Wiring"))];
        app.sessions = vec![parent, subagent("s-kid", "s-parent", "reader")];
        app.update_aggregates();
        app
    }

    fn text(app: &App) -> String {
        let session = app.sessions[0].clone();
        lines(&sidebar::panel(app, &session), 30, true)
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_panel_counts_the_todos_and_the_live_agents() {
        let app = app_with_children();
        let rendered = text(&app);
        assert!(rendered.contains("Todo 1/2"), "{rendered}");
        assert!(rendered.contains("[x] Read"), "{rendered}");
        assert!(rendered.contains("[~] Wiring"), "{rendered}");
        assert!(rendered.contains("Subagents 1/1 live"), "{rendered}");
        assert!(rendered.contains("reader"), "{rendered}");
    }

    #[test]
    fn an_empty_session_says_so_rather_than_showing_a_blank_column() {
        let mut app = App::new();
        app.sessions = vec![session("s-a", "alpha", "active", "working")];
        app.update_aggregates();
        let rendered = text(&app);
        assert!(rendered.contains("no todo list yet"), "{rendered}");
        assert!(rendered.contains("no subagents"), "{rendered}");
    }

    /// A long todo must not push the border off: the panel truncates.
    #[test]
    fn a_long_entry_is_clipped_to_the_column() {
        let mut app = app_with_children();
        app.sessions[0].todos = vec![todo("pending", &"x".repeat(200), None)];
        for line in lines(&sidebar::panel(&app, &app.sessions[0].clone()), 30, true) {
            let width: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
            assert!(width <= 30, "a sidebar line overflowed: {width}");
        }
    }

    #[test]
    fn a_child_offers_the_way_back_up() {
        let mut app = app_with_children();
        app.selected_index =
            app.flattened_sessions().iter().position(|s| s.id == "s-kid").expect("listed");
        let session = app.sessions[1].clone();
        let rendered = lines(&sidebar::panel(&app, &session), 30, true)
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("up to alpha"), "{rendered}");
    }

    #[test]
    fn a_narrow_terminal_keeps_the_transcript_instead() {
        let mut app = app_with_children();
        app.router.push(crate::app::View::Conversation);
        app.ui.sidebar_open = true;
        assert!(visible(&app, MIN_TERMINAL_WIDTH));
        assert!(!visible(&app, MIN_TERMINAL_WIDTH - 1));
        app.ui.sidebar_open = false;
        assert!(!visible(&app, 200));
    }

    /// An overlay over the conversation gets the width to itself, so no panel
    /// shows past its margins.
    #[test]
    fn an_overlay_takes_the_column_back() {
        let mut app = app_with_children();
        app.router.push(crate::app::View::Conversation);
        app.ui.sidebar_open = true;
        assert!(visible(&app, 200));
        app.router.push(crate::app::View::Sidebar);
        assert!(visible(&app, 200), "its own focus keeps it");
        app.router.push(crate::app::View::Diagnose);
        assert!(!visible(&app, 200));
    }
}
