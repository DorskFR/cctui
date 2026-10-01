//! The usage panel: pools above, credentials below.
//!
//! The gauge is drawn as text rather than with ratatui's `Gauge`, which owns a
//! whole `Rect`: a row here carries the name, the track, the readout, the
//! countdown and the burn rate on one line within the 80-column budget.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};

use cctui_clientcore::usage::HeadroomTone;

use crate::app::App;
use crate::app::usage::{AccountLine, Pane, PoolLine, WindowReadout};
use crate::theme;

/// Cells in a gauge track.
const TRACK: usize = 10;
const W_HEAD: usize = 19;
const W_LABEL: usize = 3;

const FILLED: char = '█';
const EMPTY: char = '░';

/// A `TRACK`-wide bar, or an empty track for a window that reported nothing —
/// the readout beside it is what says "not reported".
fn track(pct: Option<i64>) -> String {
    let filled = pct.map_or(0, |pct| (pct.clamp(0, 100) as usize * TRACK + 50) / 100);
    let mut out = String::with_capacity(TRACK);
    for i in 0..TRACK {
        out.push(if i < filled { FILLED } else { EMPTY });
    }
    out
}

fn tone_style(tone: HeadroomTone) -> ratatui::style::Style {
    match tone {
        HeadroomTone::Ok => theme::active(),
        HeadroomTone::Warn => theme::cost(),
        HeadroomTone::Danger => theme::error(),
        HeadroomTone::Unknown => theme::dim(),
    }
}

/// `5h ████████░░  78%  resets 1h  burn 1.4x ⚠`
fn readout_spans(readout: &WindowReadout) -> Vec<Span<'static>> {
    let mut spans = vec![
        Span::styled(format!("{:<W_LABEL$}", readout.label), theme::dim()),
        Span::styled(track(readout.pct), tone_style(readout.tone)),
        Span::styled(format!("  {:<13}", readout.value_text()), tone_style(readout.tone)),
    ];
    let burn = readout.burn_text();
    match &readout.resets {
        Some(resets) => spans.push(Span::styled(format!("resets {resets:<5}"), theme::dim())),
        // Padded only when a burn follows, so a window with neither does not
        // push what comes after off an 80-column terminal.
        None if burn.is_some() => spans.push(Span::styled(format!("{:12}", ""), theme::dim())),
        None => {}
    }
    if let Some(burn) = burn {
        let style = if readout.burning() { theme::error() } else { theme::dim() };
        let warn = if readout.burning() { " ⚠" } else { "" };
        spans.push(Span::styled(format!("burn {burn}{warn}"), style));
    }
    spans
}

fn pool_item(line: &PoolLine) -> ListItem<'static> {
    let head = line.head.clone().unwrap_or_default();
    let mut spans = vec![Span::styled(
        format!("{:<W_HEAD$} ", crate::app::session_status::truncate(&head, W_HEAD)),
        if line.head.is_some() { theme::bold() } else { theme::dim() },
    )];
    spans.extend(readout_spans(&line.readout));
    if line.head.is_some() && line.unknown_members > 0 {
        spans.push(Span::styled(format!("  {} unreadable", line.unknown_members), theme::error()));
    }
    ListItem::new(Line::from(spans))
}

fn account_item(line: &AccountLine) -> ListItem<'static> {
    let title = line.title();
    let mut spans = vec![Span::styled(
        format!("{:<W_HEAD$} ", crate::app::session_status::truncate(&title, W_HEAD)),
        theme::bold(),
    )];
    if line.readouts.is_empty() {
        spans.push(Span::styled("no windows reported", theme::dim()));
    }
    for readout in &line.readouts {
        spans.push(Span::styled(
            format!("{} {}", readout.label, readout.value_text()),
            tone_style(readout.tone),
        ));
        if readout.burning() {
            spans.push(Span::styled(" ⚠", theme::error()));
        }
        spans.push(Span::raw("  "));
    }
    if let Some(status) = &line.status {
        spans.push(Span::styled(format!(" {status}"), theme::error()));
    }
    ListItem::new(Line::from(spans))
}

fn section(title: &str, focused: bool) -> Paragraph<'static> {
    let style = if focused { theme::section_title() } else { theme::dim() };
    Paragraph::new(Line::from(Span::styled(format!(" {title}"), style)))
}

pub fn draw(frame: &mut Frame, app: &App) {
    let area = panel_area(frame.area());
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Line::from(vec![
            Span::styled(" Usage ", theme::section_title()),
            Span::styled(hint(app), theme::dim()),
        ]))
        .border_style(theme::dim());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if let Some(error) = &app.usage.error {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(format!(" {error}"), theme::error()))),
            inner,
        );
        return;
    }
    if !app.usage.loaded {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(" loading…", theme::dim()))),
            inner,
        );
        return;
    }

    let pool_lines = app.usage.pool_lines(app.clock_ms);
    let account_lines = app.usage.account_lines(app.clock_ms);
    let pools_height = u16::try_from(pool_lines.len().max(1)).unwrap_or(u16::MAX);
    let [pools_title, pools_area, accounts_title, accounts_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(pools_height.min(inner.height.saturating_sub(3))),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(inner);

    let focused = app.usage.pane;
    frame.render_widget(section("POOLS", focused == Pane::Pools), pools_title);
    draw_pane(
        frame,
        pools_area,
        pool_lines.iter().map(pool_item).collect(),
        "no pools",
        app.usage.pool_selected,
        focused == Pane::Pools,
    );
    frame.render_widget(section("ACCOUNTS", focused == Pane::Accounts), accounts_title);
    draw_pane(
        frame,
        accounts_area,
        account_lines.iter().map(account_item).collect(),
        "no accounts with usage",
        app.usage.account_selected,
        focused == Pane::Accounts,
    );
}

/// The gutter is drawn in both panes whether or not one has the keyboard, so
/// the two columns of names line up; only the focused pane shows a cursor.
fn draw_pane(
    frame: &mut Frame,
    area: Rect,
    items: Vec<ListItem<'static>>,
    empty: &str,
    selected: usize,
    focused: bool,
) {
    if items.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(format!(" {empty}"), theme::dim()))),
            area,
        );
        return;
    }
    let list = if focused {
        List::new(items).highlight_style(theme::selected()).highlight_symbol("▸")
    } else {
        List::new(items).highlight_symbol(" ")
    };
    let mut state = ListState::default().with_selected(Some(selected));
    frame.render_stateful_widget(list, area, &mut state);
}

fn hint(app: &App) -> String {
    let refreshing = if app.usage.loading { "…  " } else { "" };
    format!("  {refreshing}j/k move  Tab pane  Enter account  r refresh  Esc back ")
}

/// A margin on every side: the panel is an overlay, and the list underneath
/// showing at the edges is what says so.
fn panel_area(area: Rect) -> Rect {
    let width = area.width.saturating_sub(4).clamp(20, 96);
    let height = area.height.saturating_sub(4).max(6);
    Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::{TRACK, W_HEAD, W_LABEL, panel_area, track};
    use ratatui::layout::Rect;

    #[test]
    fn a_track_fills_in_proportion_and_never_overflows() {
        assert_eq!(track(None), "░░░░░░░░░░");
        assert_eq!(track(Some(0)), "░░░░░░░░░░");
        assert_eq!(track(Some(78)), "████████░░");
        assert_eq!(track(Some(31)), "███░░░░░░░");
        assert_eq!(track(Some(100)), "██████████");
        assert_eq!(track(Some(140)).chars().count(), TRACK, "an overage still fits the track");
    }

    #[test]
    fn a_row_fits_an_eighty_column_terminal() {
        const { assert!(W_HEAD + 1 + W_LABEL + TRACK + 2 + 13 + 12 + 11 <= 76) };
    }

    #[test]
    fn the_panel_leaves_the_list_showing_at_the_edges() {
        let area = panel_area(Rect::new(0, 0, 120, 40));
        assert!(area.width <= 96 && area.height < 40);
        assert!(area.x > 0 && area.y > 0);
        let tiny = panel_area(Rect::new(0, 0, 20, 8));
        assert!(tiny.width >= 16 && tiny.height >= 4, "it still draws on a small terminal");
    }
}
