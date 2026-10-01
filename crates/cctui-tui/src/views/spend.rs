//! The spend panel: dollars per model per window, the token windows under it,
//! and a sparkline of daily tokens.
//!
//! Every figure is formatted by `cctui_clientcore::format`, so a dollar here
//! reads exactly as the same dollar in the browser.

use cctui_clientcore::format;
use cctui_clientcore::spend::ModelSpendRow;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Sparkline};

use crate::app::App;
use crate::app::spend::{self, TokenRow};
use crate::theme;

const W_MODEL: usize = 12;
const W_MONEY: usize = 8;

pub fn draw(frame: &mut Frame, app: &App) {
    let [title_area, body_area, spark_area, footer_area, hotkeys_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    draw_title(frame, app, title_area);

    if let Some(error) = &app.spend.error {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(format!(" {error}"), theme::error()))),
            body_area,
        );
        draw_hotkeys(frame, hotkeys_area);
        return;
    }
    if !app.spend.loaded {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(" loading…", theme::dim()))),
            body_area,
        );
        draw_hotkeys(frame, hotkeys_area);
        return;
    }

    frame.render_widget(Paragraph::new(body(app)), body_area);
    draw_sparkline(frame, app, spark_area);
    draw_footer(frame, app, footer_area);
    draw_hotkeys(frame, hotkeys_area);
}

fn draw_title(frame: &mut Frame, app: &App, area: Rect) {
    let rows = spend::model_rows(app);
    let totals = spend::totals(&rows);
    let mut title = vec![
        Span::styled(" Spend", theme::section_title()),
        Span::styled(format!("  {} today", format::usd(totals.today)), theme::cost()),
        Span::styled(format!("  {} 30d", format::usd(totals.month)), theme::dim()),
    ];
    if app.spend.loading {
        title.push(Span::styled("  …", theme::dim()));
    }
    title.extend(crate::widgets::status::status_spans(app));
    frame.render_widget(Paragraph::new(Line::from(title)), area);
}

fn body(app: &App) -> Vec<Line<'static>> {
    let rows = spend::model_rows(app);
    let mut lines = vec![money_header()];
    if rows.is_empty() {
        lines.push(Line::from(Span::styled(
            " no priced sessions in the last 30 days",
            theme::dim(),
        )));
    } else {
        lines.extend(rows.iter().map(money_line));
        lines.push(total_line(&spend::totals(&rows)));
    }

    let tokens = spend::token_rows(&app.spend);
    if !tokens.is_empty() {
        lines.push(Line::from(""));
        lines.push(token_header());
        lines.extend(tokens.iter().map(token_line));
    }
    lines
}

fn money_header() -> Line<'static> {
    Line::from(Span::styled(
        format!(" {:<W_MODEL$}{:>W_MONEY$}{:>W_MONEY$}{:>W_MONEY$}", "MODEL", "TODAY", "7D", "30D"),
        theme::dim(),
    ))
}

fn money_line(row: &ModelSpendRow) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!(" {:<W_MODEL$}", clip(&row.model, W_MODEL)), theme::model()),
        Span::styled(format!("{:>W_MONEY$}", format::usd(row.today)), theme::cost()),
        Span::styled(format!("{:>W_MONEY$}", format::usd(row.week)), theme::dim()),
        Span::styled(format!("{:>W_MONEY$}", format::usd(row.month)), theme::dim()),
    ])
}

fn total_line(totals: &ModelSpendRow) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!(" {:<W_MODEL$}", "total"), theme::bold()),
        Span::styled(format!("{:>W_MONEY$}", format::usd(totals.today)), theme::bold()),
        Span::styled(format!("{:>W_MONEY$}", format::usd(totals.week)), theme::bold()),
        Span::styled(format!("{:>W_MONEY$}", format::usd(totals.month)), theme::bold()),
    ])
}

fn token_header() -> Line<'static> {
    Line::from(Span::styled(
        format!(
            " {:<W_MODEL$}{:>W_MONEY$}{:>W_MONEY$}{:>W_MONEY$}{:>W_MONEY$}",
            "TOKENS", "IN", "OUT", "CACHE", "TOTAL"
        ),
        theme::dim(),
    ))
}

fn token_line(row: &TokenRow) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!(" {:<W_MODEL$}", row.label), theme::bold()),
        Span::styled(format!("{:>W_MONEY$}", format::compact(row.input as f64)), theme::dim()),
        Span::styled(format!("{:>W_MONEY$}", format::compact(row.output as f64)), theme::dim()),
        Span::styled(format!("{:>W_MONEY$}", format::compact(row.cache_read as f64)), theme::dim()),
        Span::styled(format!("{:>W_MONEY$}", format::compact(row.total() as f64)), theme::model()),
    ])
}

fn draw_sparkline(frame: &mut Frame, app: &App, area: Rect) {
    let [label_area, bars_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);
    let series = spend::daily_series(app);
    let peak = series.iter().copied().max().unwrap_or(0);
    let label = if series.is_empty() {
        " daily tokens  no range".to_owned()
    } else {
        format!(" daily tokens  30d  peak {}", format::compact(peak as f64))
    };
    frame.render_widget(Paragraph::new(Line::from(Span::styled(label, theme::dim()))), label_area);
    if !series.is_empty() {
        frame.render_widget(
            Sparkline::default().data(&series).style(theme::model()),
            inset(bars_area),
        );
    }
}

/// One column in, so the bars line up with the tables' leading space.
const fn inset(area: Rect) -> Rect {
    Rect {
        x: area.x.saturating_add(1),
        y: area.y,
        width: area.width.saturating_sub(1),
        height: area.height,
    }
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let loss = spend::cache_loss(app);
    let line = if loss.busts == 0 {
        Line::from(Span::styled(" no prompt-cache busts in 7 days", theme::dim()))
    } else {
        Line::from(vec![
            Span::styled(" cache busts 7d  ", theme::dim()),
            Span::styled(format::usd(loss.usd), theme::error()),
            Span::styled(
                format!("  {} tok  {} busts", format::compact(loss.tokens as f64), loss.busts),
                theme::dim(),
            ),
        ])
    };
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_hotkeys(frame: &mut Frame, area: Rect) {
    let spans = vec![
        Span::styled(" r", theme::hotkey()),
        Span::styled(":refresh  ", theme::hotkey_desc()),
        Span::styled("Esc", theme::hotkey()),
        Span::styled(":back", theme::hotkey_desc()),
    ];
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn clip(s: &str, width: usize) -> String {
    crate::app::session_status::truncate(s, width)
}

/// The cost line the conversation header carries when Langfuse priced the
/// session. `None` keeps the header as it was.
#[must_use]
pub fn langfuse_span(app: &App, session_id: &str) -> Option<Span<'static>> {
    let usage = app.spend.langfuse_for(session_id)?;
    Some(Span::styled(
        format!(
            " ── {} · {} calls",
            cctui_clientcore::spend::langfuse_cost_label(usage.cost_usd),
            usage.trace_count
        ),
        theme::cost(),
    ))
}

#[cfg(test)]
mod tests {
    use super::{W_MODEL, W_MONEY};

    #[test]
    fn the_money_table_fits_in_eighty_columns() {
        const { assert!(1 + W_MODEL + 4 * W_MONEY <= 80, "the token row is the widest") };
    }
}
