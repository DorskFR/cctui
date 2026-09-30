use ratatui::text::Span;

use crate::app::App;
use crate::app::toast::Level;
use crate::theme;

fn level_style(level: Level) -> ratatui::style::Style {
    match level {
        Level::Info => theme::dim(),
        Level::Warn => theme::cost(),
        Level::Error => theme::error(),
    }
}

/// Trailing segment shared by every view's top line: the newest toast, then a
/// count of anything the TUI had to drop. Empty when there is nothing to say.
pub fn status_spans(app: &App) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    if let Some(toast) = app.toasts.latest() {
        let queued = app.toasts.queued();
        let more = if queued > 1 { format!(" +{}", queued - 1) } else { String::new() };
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!("{} {}{more}", toast.level.marker(), toast.text),
            level_style(toast.level),
        ));
    }
    if !app.status.is_clean() {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(format!("⚠ {} dropped", app.status.total()), theme::error()));
    }
    spans
}
