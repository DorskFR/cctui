use ratatui::text::Span;

use crate::app::App;
use crate::app::toast::Level;
use crate::config::chord::Chord;
use crate::config::keymap::{ActionId, Context};
use crate::theme;

/// A session waiting in another pane is only noticeable from here, so the chip
/// carries the jump key as well as the count. One chip for every needs-input
/// state: a pending approval is one of them, not a separate thing to report.
fn attention_chip(app: &App) -> Option<String> {
    let count = crate::app::attention::waiting_count(app);
    if count == 0 {
        return None;
    }
    let key = app
        .config
        .keys
        .chords_for(Context::Global, ActionId::JumpToAttention)
        .first()
        .copied()
        .map(Chord::label);
    Some(key.map_or_else(
        || format!("! {count} need input"),
        |key| format!("! {count} need input — {key}"),
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
    if let Some(chip) = attention_chip(app) {
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
    use super::{attention_chip, status_spans};
    use crate::app::App;
    use crate::testsupport::{permission_request, session};

    fn app() -> App {
        let mut app = App::new();
        app.sessions = vec![
            session("s-working", "cctui", "active", "working"),
            session("s-other", "infra", "active", "working"),
        ];
        app.update_aggregates();
        app
    }

    #[test]
    fn nothing_waiting_shows_no_chip() {
        assert!(attention_chip(&app()).is_none());
    }

    #[test]
    fn the_chip_counts_waiting_sessions_and_names_the_jump_key() {
        let mut app = app();
        app.permissions.push(permission_request());
        assert_eq!(attention_chip(&app).as_deref(), Some("! 1 need input — Ctrl+g"));

        let mut second = permission_request();
        second.session_id = "s-other".to_owned();
        second.request_id = "req-2".to_owned();
        app.permissions.push(second);
        assert_eq!(attention_chip(&app).as_deref(), Some("! 2 need input — Ctrl+g"));
        assert!(!status_spans(&app).is_empty());
    }

    /// Two requests against one session are one waiting session, not two.
    #[test]
    fn the_chip_counts_sessions_not_requests() {
        let mut app = app();
        app.permissions.push(permission_request());
        let mut again = permission_request();
        again.request_id = "req-2".to_owned();
        app.permissions.push(again);
        assert_eq!(attention_chip(&app).as_deref(), Some("! 1 need input — Ctrl+g"));
    }

    /// A blocked session is waiting even with no card of its own.
    #[test]
    fn a_blocked_session_counts_too() {
        let mut app = app();
        app.sessions[0].bucket = cctui_proto::classifier::Bucket::Blocked;
        assert_eq!(attention_chip(&app).as_deref(), Some("! 1 need input — Ctrl+g"));
    }
}
