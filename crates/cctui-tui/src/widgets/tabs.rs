//! The top-level tab bar and the summary that rides on it.

use ratatui::text::{Line, Span};

use crate::app::App;
use crate::app::slice::{self, TABS, Tab};
use crate::theme;

/// `1 Sessions  2 Overview  3 Machines`, the current one highlighted.
///
/// `reserve` is what the caller still has to fit on the row: the labels of the
/// slices you are not on go first, then whole tabs.
#[must_use]
pub fn tab_spans(app: &App, width: usize, reserve: usize) -> Vec<Span<'static>> {
    let labelled: usize = TABS.iter().map(|t: &Tab| t.label.chars().count() + 4).sum();
    let show_labels = labelled + reserve <= width;
    let groups = TABS
        .iter()
        .enumerate()
        .map(|(index, tab)| {
            let current = tab.slice == app.slice;
            let label_style = if current { theme::bold() } else { theme::dim() };
            let mut group = vec![Span::styled(format!(" {}", index + 1), theme::hotkey())];
            if current || show_labels {
                group.push(Span::styled(format!(" {}", tab.label), label_style));
            }
            group.push(Span::raw(" "));
            (current, group)
        })
        .collect();
    fit(groups, width.saturating_sub(reserve))
}

/// Drop whole tabs until what is left fits, furthest from the one you are on
/// first, so the bar always honours the width it was given and always says where
/// you are.
fn fit(mut groups: Vec<(bool, Vec<Span<'static>>)>, budget: usize) -> Vec<Span<'static>> {
    let span_width = |group: &Vec<Span<'static>>| -> usize {
        group.iter().map(|s| s.content.chars().count()).sum()
    };
    let total = |groups: &[(bool, Vec<Span<'static>>)]| -> usize {
        groups.iter().map(|(_, g)| span_width(g)).sum()
    };
    while total(&groups) > budget && groups.len() > 1 {
        let current = groups.iter().position(|(is, _)| *is).unwrap_or(0);
        // Whichever end is further from the current tab gives one up.
        let drop_at = if current >= groups.len() - 1 - current { 0 } else { groups.len() - 1 };
        if groups[drop_at].0 {
            break;
        }
        groups.remove(drop_at);
    }
    // Nothing left to drop but the current tab's own label: the number alone
    // still teaches the key, and the row has to fit whatever it was given.
    if total(&groups) > budget
        && let Some((_, group)) = groups.iter_mut().find(|(is, _)| *is)
        && group.len() > 2
    {
        group.remove(1);
    }
    groups.into_iter().flat_map(|(_, g)| g).collect()
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
    let word = |w: &str| if width < COMPACT_UNDER { String::new() } else { format!(" {w}") };
    let mut spans = vec![
        Span::styled(format!("● {mark}{}{}", s.live, word("live")), theme::active()),
        Span::raw("  "),
        Span::styled(
            format!("▪ {}/{}{}", s.machines_online, s.machines_total, word("machines")),
            theme::dim(),
        ),
        Span::raw("  "),
        Span::styled(
            format!("{}{}", cctui_clientcore::usage::money(s.today_cost_usd), word("today")),
            theme::cost(),
        ),
    ];
    if s.unread > 0 {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(format!("●{}{}", s.unread, word("unread")), theme::unread()));
    }
    spans
}

#[must_use]
pub fn tab_line(app: &App, width: usize) -> Line<'static> {
    Line::from(tab_spans(app, width, 0))
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
        assert_eq!(text(&tab_spans(&app, 120, 0)), " 1 Sessions  2 Overview  3 Machines ");
    }

    #[test]
    fn a_row_with_other_work_on_it_keeps_only_the_current_label() {
        let app = app();
        let bare = text(&tab_spans(&app, 120, 100));
        assert_eq!(bare, " 1 Sessions  2  3 ");
        assert!(bare.chars().count() + 100 <= 120, "it has to actually fit: {bare:?}");
    }

    /// However tight the row, the bar fits what it was given and still shows the
    /// number of the slice you are on — the label is the last thing to go.
    #[test]
    fn a_row_with_almost_nothing_left_still_fits_and_names_the_key() {
        let mut app = app();
        app.slice = crate::app::slice::Slice::Machines;
        // 77 leaves exactly the three columns a bare number needs; below that
        // there is no row left to put a tab on.
        for reserve in [0, 40, 60, 70, 76, 77] {
            let bar = text(&tab_spans(&app, 80, reserve));
            assert!(
                bar.chars().count() + reserve <= 80,
                "reserve {reserve} overflowed with {bar:?}"
            );
            assert!(bar.contains('3'), "reserve {reserve} lost the current tab: {bar:?}");
        }
        assert!(text(&tab_spans(&app, 80, 40)).contains("Machines"), "the label survives a bit");
    }

    #[test]
    fn the_summary_leaves_the_needs_input_count_to_the_attention_chip() {
        let mut app = app();
        app.sessions[0].attention = Some(cctui_proto::models::Attention::NeedsInput);
        let line = text(&summary_spans(&app, 120));
        assert!(!line.contains("need"), "one count, one place: {line}");
    }

    #[test]
    fn unread_shows_only_when_there_is_some() {
        let mut app = app();
        assert!(!text(&summary_spans(&app, 120)).contains("unread"));
        app.sessions[0].unread_count = 5;
        assert!(text(&summary_spans(&app, 120)).contains("●5 unread"));
    }

    #[test]
    fn the_current_tab_is_the_one_the_slice_names() {
        let mut app = app();
        reduce(&mut app, Action::Slice(SliceAction::Switch(2)));
        assert_eq!(app.slice, Slice::Overview);
        let spans = tab_spans(&app, 120, 0);
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
        assert!(!served.contains('~'), "{served}");
    }
}
