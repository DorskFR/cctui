//! The attachment chip row and the path prompt.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::theme;

/// The paperclip a theme with glyphs uses; the ASCII fallback is `+`.
const CLIP: &str = "📎";
const CLIP_ASCII: &str = "+";

/// Rows the chip line needs: one when anything is staged or refused, else none.
#[must_use]
pub fn height(app: &App, session_id: &str) -> u16 {
    u16::from(app.attachments.get(session_id).is_some())
}

/// `📎 [paste-1.txt 12 KB] [design.pdf 1.2 MB]`, with the focused chip inverted
/// and a cap breach in its place.
pub fn draw_chips(frame: &mut Frame, app: &App, session_id: &str, area: Rect) {
    let Some(stage) = app.attachments.get(session_id) else { return };
    let lead = if app.config.prefs.ascii_glyphs { CLIP_ASCII } else { CLIP };
    let mut spans = vec![Span::styled(format!(" {lead} "), theme::dim())];

    if let Some(error) = &stage.error {
        spans.push(Span::styled(error.clone(), theme::error()));
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
        return;
    }

    for (at, item) in stage.items.iter().enumerate() {
        let focused = app.attachments.chip_cursor == Some(at);
        // The marker, not just the style: a chip about to be deleted by
        // Backspace has to be obvious on a monochrome terminal too.
        let (open, close) = if focused { ("‹", "›") } else { ("[", "]") };
        let style = if focused { theme::selected() } else { theme::branch() };
        spans.push(Span::styled(format!("{open}{}{close}", item.chip_label()), style));
        spans.push(Span::raw(" "));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}
