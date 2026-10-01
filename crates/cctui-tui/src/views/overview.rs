//! The Overview slice: the web UI's metric tiles, as rows.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::{App, slice};
use crate::theme;

/// One tile: a figure, what it counts, and whether it wants attention.
struct Tile {
    value: String,
    label: &'static str,
    warn: bool,
}

/// The four headline tiles, in the web UI's display order.
fn tiles(app: &App) -> Vec<Tile> {
    let s = slice::summary(app);
    let stats = app.stats.as_ref();
    let mut out = vec![
        Tile { value: s.live.to_string(), label: "live", warn: false },
        Tile { value: s.needs_input.to_string(), label: "need input", warn: s.needs_input > 0 },
        Tile {
            value: format!("{}/{}", s.machines_online, s.machines_total),
            label: "machines online",
            warn: false,
        },
    ];
    if let Some(stats) = stats {
        out.push(Tile {
            value: format!("{} ({} archived)", stats.total, stats.archived),
            label: "sessions",
            warn: false,
        });
        out.push(Tile {
            value: format!("{} / {} / {}", stats.today, stats.week, stats.month),
            label: "registered today / week / month",
            warn: false,
        });
    }
    out.push(Tile {
        value: format!("${:.2}", s.today_cost_usd),
        label: "cost of today's sessions",
        warn: false,
    });
    out
}

fn tile_lines(app: &App, width: usize) -> Vec<Line<'static>> {
    const VALUE_COLS: usize = 18;
    let mut lines = Vec::new();
    for tile in tiles(app) {
        let value = crate::app::session_status::truncate(&tile.value, VALUE_COLS);
        let pad = VALUE_COLS.saturating_sub(value.chars().count()) + 1;
        let room = width.saturating_sub(VALUE_COLS + 2);
        lines.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(value, if tile.warn { theme::attention() } else { theme::bold() }),
            Span::raw(" ".repeat(pad)),
            Span::styled(crate::app::session_status::truncate(tile.label, room), theme::dim()),
        ]));
    }
    lines
}

pub fn draw(frame: &mut Frame, app: &App) {
    let [status_area, tabs_area, body_area, hotkeys_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    let mut status = vec![
        Span::styled(" cctui ", theme::status_bar_bg()),
        Span::raw(" "),
        Span::styled(format!("v{}", app.version), theme::dim()),
        Span::raw("  "),
    ];
    status.extend(crate::widgets::tabs::summary_spans(app, usize::from(status_area.width)));
    status.extend(crate::widgets::status::status_spans(app));
    frame.render_widget(Paragraph::new(Line::from(status)), status_area);

    frame.render_widget(Paragraph::new(crate::widgets::tabs::tab_line(app)), tabs_area);

    draw_tiles(frame, app, body_area);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" j/k ", theme::hotkey()),
            Span::raw("Scroll  "),
            Span::styled("r ", theme::hotkey()),
            Span::raw("Refresh  "),
            Span::styled("1-9 ", theme::hotkey()),
            Span::raw("Switch view  "),
            Span::styled("? ", theme::hotkey()),
            Span::raw("Help  "),
            Span::styled("q ", theme::hotkey()),
            Span::raw("Quit"),
        ])),
        hotkeys_area,
    );
}

fn draw_tiles(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::border_dim())
        .title(" Overview ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let lines = tile_lines(app, usize::from(inner.width));
    let height = usize::from(inner.height);
    let offset = app.overview_scroll.min(lines.len().saturating_sub(height));
    let shown: Vec<Line> = lines.into_iter().skip(offset).take(height).collect();
    frame.render_widget(Paragraph::new(shown).style(Style::new()), inner);
}

#[cfg(test)]
mod tests {
    use super::tiles;
    use crate::app::slice::SliceAction;
    use crate::app::state::App;
    use crate::app::{Action, reduce};
    use crate::testsupport::{CLOCK_MS, session};

    fn app() -> App {
        let mut app = App::new();
        app.clock_ms = CLOCK_MS;
        app.sessions = vec![session("s-a", "alpha", "active", "working")];
        app.update_aggregates();
        app
    }

    #[test]
    fn the_first_three_tiles_mirror_the_web_uis_order() {
        let app = app();
        let tiles = tiles(&app);
        assert_eq!(tiles[0].label, "live");
        assert_eq!(tiles[1].label, "need input");
        assert_eq!(tiles[2].label, "machines online");
    }

    #[test]
    fn the_served_tiles_appear_only_once_the_stats_land() {
        let mut app = app();
        let before = tiles(&app).len();
        reduce(
            &mut app,
            Action::Slice(SliceAction::StatsLoaded(Box::new(cctui_proto::api::SessionStats {
                total: 12,
                live: 3,
                needs_input: 0,
                archived: 4,
                today: 5,
                yesterday: 1,
                week: 9,
                month: 11,
            }))),
        );
        let after = tiles(&app);
        assert!(after.len() > before, "the counts the server owns add tiles");
        assert!(after.iter().any(|t| t.value == "12 (4 archived)"));
        assert!(after.iter().any(|t| t.value == "5 / 9 / 11"));
    }

    #[test]
    fn a_session_wanting_the_operator_makes_its_tile_warn() {
        let mut app = app();
        assert!(!tiles(&app)[1].warn);
        app.sessions[0].attention = Some(cctui_proto::models::Attention::NeedsInput);
        assert!(tiles(&app)[1].warn);
    }
}
