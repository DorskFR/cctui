//! The top-level tab bar and the summary that rides on it.

use ratatui::text::{Line, Span};

use crate::app::App;
use crate::app::slice::{self, TABS};
use crate::theme;

/// `1 Sessions  2 Bookmarks  3 Overview  …`, the current one highlighted and
/// the ones the TUI has not built yet dimmed but still numbered.
#[must_use]
pub fn tab_spans(app: &App) -> Vec<Span<'static>> {
    let mut spans = Vec::with_capacity(TABS.len() * 3);
    for (index, tab) in TABS.iter().enumerate() {
        let number = index + 1;
        let current = tab.slice == Some(app.slice);
        let (key_style, label_style) = if current {
            (theme::hotkey(), theme::bold())
        } else if tab.slice.is_some() {
            (theme::hotkey(), theme::dim())
        } else {
            (theme::border_dim(), theme::border_dim())
        };
        spans.push(Span::styled(format!(" {number}"), key_style));
        spans.push(Span::styled(format!(" {}", tab.label), label_style));
        spans.push(Span::raw(" "));
    }
    spans
}

/// Below this the summary drops its words: the status line also carries the
/// pending-approval chip and the newest toast, and those say more.
const COMPACT_UNDER: usize = 100;

/// `● 3 live  ⚠ 2 need you  ▪ 1/2 machines  $4.20 today`, with a `~` while the
/// server's own counts are still in flight.
#[must_use]
pub fn summary_spans(app: &App, width: usize) -> Vec<Span<'static>> {
    let s = slice::summary(app);
    let mark = if s.pending { "~" } else { "" };
    let needs_style = if s.needs_input > 0 { theme::attention() } else { theme::dim() };
    let word = |w: &str| if width < COMPACT_UNDER { String::new() } else { format!(" {w}") };
    vec![
        Span::styled(format!("● {mark}{}{}", s.live, word("live")), theme::active()),
        Span::raw("  "),
        Span::styled(format!("⚠ {mark}{}{}", s.needs_input, word("need you")), needs_style),
        Span::raw("  "),
        Span::styled(
            format!("▪ {}/{}{}", s.machines_online, s.machines_total, word("machines")),
            theme::dim(),
        ),
        Span::raw("  "),
        Span::styled(format!("${:.2}{}", s.today_cost_usd, word("today")), theme::cost()),
    ]
}

#[must_use]
pub fn tab_line(app: &App) -> Line<'static> {
    Line::from(tab_spans(app))
}

#[cfg(test)]
mod tests {
    use super::{summary_spans, tab_spans};
    use crate::app::slice::{Slice, SliceAction};
    use crate::app::state::App;
    use crate::app::{Action, reduce};
    use crate::testsupport::{CLOCK_MS, session};

    fn text(spans: &[ratatui::text::Span<'static>]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn app() -> App {
        let mut app = App::new();
        app.clock_ms = CLOCK_MS;
        app.sessions = vec![session("s-a", "alpha", "active", "working")];
        app.update_aggregates();
        app
    }

    #[test]
    fn every_tab_is_numbered_in_order() {
        let app = app();
        assert_eq!(
            text(&tab_spans(&app)),
            " 1 Sessions  2 Bookmarks  3 Overview  4 Access  5 Accounts  6 Settings "
        );
    }

    #[test]
    fn the_current_tab_is_the_one_the_slice_names() {
        let mut app = app();
        reduce(&mut app, Action::Slice(SliceAction::Switch(3)));
        assert_eq!(app.slice, Slice::Overview);
        let spans = tab_spans(&app);
        let overview = spans
            .iter()
            .position(|s| s.content.as_ref() == " Overview")
            .expect("the overview label");
        assert_eq!(spans[overview].style, crate::theme::bold(), "the current tab is bold");
    }

    #[test]
    fn the_summary_marks_itself_until_the_server_answers() {
        let app = app();
        let pending = text(&summary_spans(&app, 120));
        assert!(pending.contains("~1 live"), "{pending}");
        assert!(pending.contains("machines"));
        assert!(pending.contains("today"));
    }

    #[test]
    fn the_served_counts_drop_the_mark() {
        let mut app = app();
        reduce(
            &mut app,
            Action::Slice(SliceAction::StatsLoaded(Box::new(cctui_proto::api::SessionStats {
                total: 9,
                live: 4,
                needs_input: 1,
                archived: 2,
                today: 3,
                yesterday: 0,
                week: 5,
                month: 7,
            }))),
        );
        let served = text(&summary_spans(&app, 120));
        assert!(served.contains("● 4 live"), "{served}");
        assert!(served.contains("⚠ 1 need you"), "{served}");
        assert!(!served.contains('~'), "{served}");
    }
}
