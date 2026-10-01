//! The one status row between the transcript and the composer.

use cctui_proto::api::SessionListItem;
use cctui_proto::session_end::EndTone;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::app::attention::{Activity, EndBadge, activity};
use crate::theme;

const SPINNER: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];

pub fn draw(frame: &mut Frame, area: Rect, app: &App, session: &SessionListItem) {
    frame.render_widget(Paragraph::new(line(app, session)), area);
}

/// An interrupt in flight, or a key waiting for its second press, outranks the
/// activity: both are answers to something the operator just did.
pub fn line(app: &App, session: &SessionListItem) -> Line<'static> {
    if app.controls.is_interrupting(&session.id) {
        return Line::from(vec![
            Span::styled(" ⏹ ", theme::cost()),
            Span::styled("interrupting…", theme::cost()),
        ]);
    }
    if let Some(what) = app.controls.armed_for(&session.id, app.clock_ms) {
        return Line::from(vec![
            Span::styled(" ⚠ ", theme::cost()),
            Span::styled(what.hint().to_owned(), theme::cost()),
        ]);
    }
    Line::from(spans(&activity(app, session), app.clock_ms))
}

fn spans(state: &Activity, clock_ms: i64) -> Vec<Span<'static>> {
    match state {
        Activity::Ended(badge) => ended(badge),
        Activity::NeedsInput => vec![
            Span::styled(" ◆ ", theme::cost()),
            Span::styled("waiting for input", theme::cost()),
        ],
        Activity::Working { detail, age_secs } => working(detail.as_deref(), *age_secs, clock_ms),
        Activity::Silent { secs } => vec![
            Span::styled(" ⚠ ", theme::cost()),
            Span::styled(format!("silent for {}", elapsed(*secs)), theme::cost()),
        ],
        Activity::Idle => {
            vec![Span::styled(" ○ ", theme::dim()), Span::styled("idle", theme::dim())]
        }
    }
}

fn ended(badge: &EndBadge) -> Vec<Span<'static>> {
    let style = if badge.muted { theme::dim() } else { tone_style(badge.tone) };
    let mut spans =
        vec![Span::styled(" ■ ", style), Span::styled(format!("ended · {}", badge.label), style)];
    if let Some(detail) = &badge.detail {
        spans.push(Span::styled(format!(" — {detail}"), theme::dim()));
    }
    // The action the ended state was always meant to offer.
    spans.push(Span::styled("  r ", theme::hotkey()));
    spans.push(Span::styled("resume", theme::hotkey_desc()));
    spans
}

fn working(detail: Option<&str>, age_secs: Option<i64>, clock_ms: i64) -> Vec<Span<'static>> {
    let frame = SPINNER[((clock_ms / 120).unsigned_abs() as usize) % SPINNER.len()];
    let mut spans =
        vec![Span::styled(format!(" {frame} "), theme::active()), Span::raw("working".to_owned())];
    if let Some(detail) = detail {
        spans.push(Span::styled(format!(" · {detail}"), theme::dim()));
    }
    if let Some(secs) = age_secs {
        spans.push(Span::styled(format!("  {}", elapsed(secs)), theme::dim()));
    }
    spans
}

/// Tones are the domain's; which colour each one wears is the terminal's.
fn tone_style(tone: EndTone) -> Style {
    match tone {
        EndTone::Ok => theme::active(),
        EndTone::Warn => theme::cost(),
        EndTone::Danger => theme::error(),
        EndTone::Neutral | EndTone::Info => theme::dim(),
    }
}

fn elapsed(secs: i64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    }
}

#[cfg(test)]
mod tests {
    use cctui_proto::models::SessionEndReason;

    use super::{elapsed, line};
    use crate::app::App;
    use crate::testsupport::session;

    fn rendered(app: &App) -> String {
        line(app, &app.sessions[0]).spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn app() -> App {
        let mut app = App::new();
        app.sessions = vec![session("s-a", "alpha", "active", "working")];
        app.update_aggregates();
        app
    }

    #[test]
    fn a_working_session_spins_with_its_current_step() {
        let mut app = app();
        app.clock_ms = 120_000;
        app.sessions[0].activity_detail = Some("running the tests".to_owned());
        app.sessions[0].last_activity_at = chrono::DateTime::from_timestamp_millis(45_000);
        let text = rendered(&app);
        assert!(text.contains("working · running the tests"), "{text}");
        assert!(text.contains("1m15s"), "{text}");
    }

    #[test]
    fn a_blocked_session_says_it_is_waiting() {
        let mut app = app();
        app.sessions[0].bucket = cctui_proto::classifier::Bucket::Blocked;
        assert!(rendered(&app).contains("waiting for input"));
    }

    #[test]
    fn a_quiet_session_reports_how_long_it_has_been_quiet() {
        let mut app = app();
        app.clock_ms = 600_000;
        app.sessions[0].last_activity_at = chrono::DateTime::from_timestamp_millis(300_000);
        assert!(rendered(&app).contains("silent for 5m0s"));
    }

    #[test]
    fn an_ended_session_names_the_reason_and_offers_to_resume_it() {
        let mut app = app();
        app.sessions[0].end_reason = Some(SessionEndReason::Crashed);
        let text = rendered(&app);
        assert!(text.contains("ended · crashed"), "{text}");
        assert!(text.contains("r resume"), "the ended state offers its action: {text}");
    }

    #[test]
    fn a_failed_start_carries_its_detail() {
        let mut app = app();
        app.sessions[0].end_reason = Some(SessionEndReason::SpawnFailed);
        app.sessions[0].end_detail = Some("unknown model gpt-nope".to_owned());
        let text = rendered(&app);
        assert!(text.contains("ended · failed"), "{text}");
        assert!(text.contains("unknown model gpt-nope"), "{text}");
    }

    #[test]
    fn the_spinner_advances_with_the_clock() {
        let mut app = app();
        app.sessions[0].activity_detail = Some("step".to_owned());
        app.clock_ms = 0;
        let first = rendered(&app);
        app.clock_ms = 120;
        assert_ne!(first, rendered(&app));
    }

    #[test]
    fn durations_read_in_the_largest_useful_unit() {
        assert_eq!(elapsed(9), "9s");
        assert_eq!(elapsed(75), "1m15s");
        assert_eq!(elapsed(7_260), "2h1m");
    }
}
