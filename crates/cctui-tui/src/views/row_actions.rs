//! The list's row-action strip: select mode, the confirm prompt, the rename
//! field. It takes the bottom line, replacing the hotkey hints while it is up.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::theme;

/// Draws the strip and reports whether it claimed the line.
pub fn draw_strip(frame: &mut Frame, app: &App, area: Rect) -> bool {
    let Some(text) = app.row_actions.strip() else { return false };
    let style =
        if app.row_actions.confirm.is_some() { theme::attention() } else { theme::hotkey() };
    frame.render_widget(Paragraph::new(Line::from(Span::styled(text, style))), area);
    true
}

/// The checkbox a row carries in select mode, `None` outside it.
#[must_use]
pub fn checkbox(app: &App, session_id: &str) -> Option<&'static str> {
    if !app.row_actions.mode {
        return None;
    }
    Some(if app.row_actions.is_selected(session_id) { "[x] " } else { "[ ] " })
}
