use ratatui::text::Span;

use crate::app::App;
use crate::app::toast::Level;
use crate::config::chord::Chord;
use crate::config::keymap::{ActionId, Context};
use crate::theme;

/// A request waiting in another session is only noticeable from here, so the
/// chip carries the jump key as well as the count.
fn pending_chip(app: &App) -> Option<String> {
    let count = app.permissions.len();
    if count == 0 {
        return None;
    }
    let plural = if count == 1 { "" } else { "s" };
    let key = app
        .config
        .keys
        .chords_for(Context::Global, ActionId::JumpToPending)
        .first()
        .copied()
        .map(Chord::label);
    Some(key.map_or_else(
        || format!("⚠ {count} approval{plural} pending"),
        |key| format!("⚠ {count} approval{plural} pending — {key} to jump"),
    ))
}

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
    if let Some(chip) = pending_chip(app) {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(chip, theme::cost()));
    }
    if let Some(chip) = app.auth.chip() {
        spans.push(Span::raw("  "));
        let style = if chip.rejected { theme::error() } else { theme::dim() };
        spans.push(Span::styled(format!("@{}", chip.text), style));
    }
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

#[cfg(test)]
mod tests {
    use super::{pending_chip, status_spans};
    use crate::app::App;
    use crate::testsupport::permission_request;

    #[test]
    fn nothing_pending_shows_no_chip() {
        assert!(pending_chip(&App::new()).is_none());
    }

    #[test]
    fn the_chip_counts_requests_and_names_the_jump_key() {
        let mut app = App::new();
        app.permissions.push(permission_request());
        assert_eq!(pending_chip(&app).as_deref(), Some("⚠ 1 approval pending — Ctrl+g to jump"));

        let mut second = permission_request();
        second.session_id = "s-other".to_owned();
        second.request_id = "req-2".to_owned();
        app.permissions.push(second);
        assert!(pending_chip(&app).expect("a chip").starts_with("⚠ 2 approvals pending"));
        assert!(!status_spans(&app).is_empty());
    }
}
