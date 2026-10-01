//! The accounts slice: the identity table, an optional detail pane, and the
//! pools pane under them.
//!
//! Fixed columns, elastic name, cut rather than wrapped: the table has to stay
//! a table in 80 columns.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};

use crate::app::accounts::{Focus, Mode, kinds_text, weight_text};
use crate::app::pools::{Field, Mode as PoolMode, STRATEGIES};
use crate::app::{App, pools};
use crate::theme;
use crate::widgets::picker::{Picker, PickerRow};

const W_KIND: usize = 14;
const W_POOL: usize = 14;
const W_USAGE: usize = 9;

/// Everything but the elastic name column, plus its trailing gap.
const FIXED_COLS: usize = W_KIND + W_POOL + W_USAGE + 2;

/// The pools pane gets a header plus this many lines before it scrolls.
const POOL_LINES: u16 = 6;

pub fn draw(frame: &mut Frame, app: &App) {
    let [title_area, body_area, hotkeys_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1), Constraint::Length(1)])
            .areas(frame.area());

    draw_title(frame, app, title_area);

    let detail_height = if app.accounts.detail { 7 } else { 0 };
    let [list_area, detail_area, pools_area] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(detail_height),
        Constraint::Length(POOL_LINES + 1),
    ])
    .areas(body_area);

    draw_list(frame, app, list_area);
    if app.accounts.detail {
        draw_detail(frame, app, detail_area);
    }
    draw_pools(frame, app, pools_area);
    draw_hotkeys(frame, app, hotkeys_area);
    draw_modal(frame, app);
}

fn draw_title(frame: &mut Frame, app: &App, area: Rect) {
    let (poolable, total) = app.accounts.counts();
    let mut title = vec![
        Span::styled(" Accounts", theme::section_title()),
        Span::styled(format!("  {poolable}/{total} poolable"), theme::dim()),
    ];
    if app.accounts.read_only {
        title.push(Span::styled("  read-only", theme::stale()));
    }
    if app.accounts.loading {
        title.push(Span::styled("  …", theme::dim()));
    }
    title.extend(crate::widgets::status::status_spans(app));
    frame.render_widget(Paragraph::new(Line::from(title)), area);
}

/// Whatever the fixed columns and the cursor gutter leave, within reason.
fn name_width(area_width: u16) -> usize {
    let usable = usize::from(area_width).saturating_sub(FIXED_COLS + 2);
    usable.clamp(8, 20)
}

fn draw_list(frame: &mut Frame, app: &App, area: Rect) {
    let [header_area, rows_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);
    let name_width = name_width(rows_area.width);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(
                "    {:<name_width$}  {:<W_KIND$}{:<W_POOL$}{}",
                "NAME", "PROVIDERS", "POOL", "USAGE"
            ),
            theme::dim(),
        ))),
        header_area,
    );

    if let Some(error) = &app.accounts.error {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(format!(" {error}"), theme::error()))),
            rows_area,
        );
        return;
    }
    if app.accounts.rows.is_empty() {
        let text = if app.accounts.loaded { " no accounts" } else { " loading…" };
        frame
            .render_widget(Paragraph::new(Line::from(Span::styled(text, theme::dim()))), rows_area);
        return;
    }

    let items: Vec<ListItem> =
        app.accounts.rows.iter().map(|row| line(app, row, name_width)).collect();
    let focused = matches!(app.accounts.focus, Focus::Accounts);
    let list = List::new(items)
        .highlight_style(if focused { theme::selected() } else { theme::dim() })
        .highlight_symbol("▸ ");
    let mut state = ListState::default().with_selected(Some(app.accounts.selected));
    frame.render_stateful_widget(list, rows_area, &mut state);
}

fn line<'a>(app: &App, row: &'a cctui_client::Account, name_width: usize) -> ListItem<'a> {
    let withheld = !row.pool_eligible;
    let name_style = if withheld { theme::dim() } else { theme::bold() };
    let glyph = if row.pool_eligible { "●" } else { "○" };
    let pool = pools::name_of(app, &row.id).map_or_else(
        || "-".to_owned(),
        |name| format!("pool:{}", crate::app::session_status::truncate(&name, W_POOL - 6)),
    );
    let usage = usage_cell(app, &row.id);
    let name = crate::app::session_status::truncate(&row.name, name_width);

    let mut spans = vec![
        Span::styled(format!("{glyph} "), if withheld { theme::dim() } else { theme::active() }),
        Span::styled(format!("{name:<name_width$}  "), name_style),
        Span::styled(
            format!(
                "{:<W_KIND$}",
                crate::app::session_status::truncate(&kinds_text(row), W_KIND - 1)
            ),
            theme::dim(),
        ),
        Span::styled(format!("{pool:<W_POOL$}"), theme::dim()),
        Span::styled(format!("{usage:<W_USAGE$}"), theme::dim()),
    ];
    let weight = weight_text(row);
    if !weight.is_empty() {
        spans.push(Span::styled(format!("{weight} "), theme::stale()));
    }
    let chips = app.accounts.chips(&row.id);
    if let Some(chip) = chips.first() {
        spans.push(Span::styled(format!("→ {}", chip.target_name), theme::stale()));
    }
    if row.providers.iter().any(|p| p.needs_reauth) {
        spans.push(Span::styled(" reauth", theme::error()));
    }
    ListItem::new(Line::from(spans))
}

/// The level only: the usage lane's full reading carries a reset time too, which
/// belongs in the detail pane rather than in a 9-column cell.
fn usage_cell(app: &App, account_id: &str) -> String {
    let Some(text) = crate::app::accounts::usage_text(app, account_id) else {
        return "-".to_owned();
    };
    let level = text.split(" · ").next().unwrap_or(&text);
    crate::app::session_status::truncate(level, W_USAGE - 1)
}

fn draw_detail(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default().borders(Borders::ALL).border_style(theme::border_focused());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(row) = app.accounts.selected_row() else { return };

    let mut lines = vec![Line::from(vec![
        Span::styled(row.name.clone(), theme::bold()),
        Span::styled(
            format!("  owner:{}", row.user_name.clone().unwrap_or_else(|| "you".to_owned())),
            theme::dim(),
        ),
        Span::styled(format!("  weight {:.2}", row.pool_weight), theme::dim()),
        Span::styled(
            if row.pool_eligible { "  poolable" } else { "  withheld from pools" },
            theme::stale(),
        ),
    ])];
    for provider in &row.providers {
        let mut spans = vec![
            Span::styled(
                format!("  {:<22}", cctui_proto::provider::provider_label(&provider.provider)),
                theme::bold(),
            ),
            Span::styled(format!("{:<12}", provider.family), theme::dim()),
            Span::styled(
                format!("${:.2}  {} tok", provider.est_cost_usd, provider.total_tokens),
                theme::dim(),
            ),
        ];
        if provider.needs_reauth {
            spans.push(Span::styled("  reauthenticate", theme::error()));
        }
        lines.push(Line::from(spans));
    }
    // The gauges belong to the usage lane; this pane shows its one-line reading.
    let usage = crate::app::accounts::usage_text(app, &row.id).map_or_else(
        || "  usage: not read yet — Ctrl+u".to_owned(),
        |text| format!("  usage: {text}"),
    );
    lines.push(Line::from(Span::styled(usage, theme::dim())));
    for chip in app.accounts.chips(&row.id) {
        lines.push(Line::from(Span::styled(
            format!("  {} → {}", chip.family, chip.target_name),
            theme::stale(),
        )));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_pools(frame: &mut Frame, app: &App, area: Rect) {
    let focused = matches!(app.accounts.focus, Focus::Pools);
    let [header_area, rows_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" Pools", if focused { theme::section_title() } else { theme::dim() }),
            Span::styled(format!("  {}", app.pools.rows.len()), theme::dim()),
        ])),
        header_area,
    );

    if let Some(error) = &app.pools.error {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(format!(" {error}"), theme::error()))),
            rows_area,
        );
        return;
    }
    let entries = app.pools.entries();
    if entries.is_empty() {
        let text = if app.pools.loaded { " no pools — n to add one" } else { " loading…" };
        frame
            .render_widget(Paragraph::new(Line::from(Span::styled(text, theme::dim()))), rows_area);
        return;
    }

    let items: Vec<ListItem> = entries.iter().map(|entry| pool_line(app, *entry)).collect();
    let list = List::new(items)
        .highlight_style(if focused { theme::selected() } else { theme::dim() })
        .highlight_symbol("▸ ");
    let mut state = ListState::default().with_selected(Some(app.pools.cursor));
    frame.render_stateful_widget(list, rows_area, &mut state);
}

fn pool_line(app: &App, entry: pools::Entry) -> ListItem<'static> {
    let Some(pool) = app.pools.rows.get(entry.pool) else { return ListItem::new("") };
    match entry.member {
        None => ListItem::new(Line::from(vec![
            Span::styled(format!(" {}", pool.name), theme::bold()),
            Span::styled(format!("  {}", pool.strategy), theme::dim()),
            Span::styled(if pool.failover { "  failover" } else { "" }, theme::stale()),
            Span::styled(format!("  {} members", pool.members.len()), theme::dim()),
        ])),
        Some(index) => {
            let members = app.pools.ordered(entry.pool);
            let Some(member) = members.get(index) else { return ListItem::new("") };
            let mut spans = vec![
                Span::styled(format!("   {}. ", index + 1), theme::dim()),
                Span::styled(member.name.clone(), theme::bold()),
            ];
            if !member.owned {
                spans.push(Span::styled("  shared", theme::dim()));
            }
            if !member.pool_eligible {
                spans.push(Span::styled("  withheld — never elected", theme::stale()));
            }
            ListItem::new(Line::from(spans))
        }
    }
}

fn draw_hotkeys(frame: &mut Frame, app: &App, area: Rect) {
    let pairs: &[(&str, &str)] = if matches!(app.accounts.focus, Focus::Pools) {
        &[
            ("j/k", "move"),
            ("a", "add"),
            ("d", "drop"),
            ("J/K", "reorder"),
            ("n", "new pool"),
            ("D", "delete"),
            ("Tab", "accounts"),
        ]
    } else {
        &[
            ("j/k", "move"),
            ("Enter", "detail"),
            ("e", "poolable"),
            ("+/-", "weight"),
            ("R", "reset"),
            ("x", "redirect"),
            ("Tab", "pools"),
        ]
    };
    let mut spans = Vec::new();
    for (key, label) in pairs {
        let prefix = if spans.is_empty() { " " } else { "  " };
        spans.push(Span::styled(format!("{prefix}{key}"), theme::hotkey()));
        spans.push(Span::styled(format!(":{label}"), theme::hotkey_desc()));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_modal(frame: &mut Frame, app: &App) {
    match (&app.accounts.mode, &app.pools.mode) {
        (Some(Mode::Redirect { account, targets, selected }), _) => {
            let rows: Vec<PickerRow> = targets
                .iter()
                .map(|target| PickerRow {
                    lead: target.families.join("+"),
                    text: target.name.clone(),
                })
                .collect();
            crate::widgets::picker::draw(
                frame,
                &Picker {
                    title: &format!("Redirect {account}'s launches to"),
                    filter: None,
                    prompt: "",
                    rows: &rows,
                    selected: *selected,
                    empty: "no account shares a provider family",
                    hotkeys: &[("Enter", "redirect"), ("Esc", "cancel")],
                },
            );
        }
        (Some(Mode::ConfirmReset { account, credit, .. }), _) => confirm(
            frame,
            "Claim a reset",
            &[
                format!("Spend a usage-limit reset on {account}?"),
                String::new(),
                format!("credit: {credit}"),
            ],
        ),
        (None, Some(PoolMode::AddMember { pool_name, candidates, selected, .. })) => {
            let rows: Vec<PickerRow> =
                candidates.iter().map(|c| PickerRow::plain(c.name.clone())).collect();
            crate::widgets::picker::draw(
                frame,
                &Picker {
                    title: &format!("Add an account to {pool_name}"),
                    filter: None,
                    prompt: "",
                    rows: &rows,
                    selected: *selected,
                    empty: "no eligible account",
                    hotkeys: &[("Enter", "add"), ("Esc", "cancel")],
                },
            );
        }
        (None, Some(PoolMode::New { name, strategy, field })) => {
            let mark = |own: Field| if *field == own { "▏" } else { "" };
            confirm(
                frame,
                "New pool",
                &[
                    format!("name      {name}{}", mark(Field::Name)),
                    format!("strategy  {}{}", STRATEGIES[*strategy], mark(Field::Strategy)),
                    String::new(),
                    "Tab moves fields and cycles the strategy".to_owned(),
                ],
            );
        }
        (None, Some(PoolMode::ConfirmDelete { name, typed, .. })) => {
            confirm(
                frame,
                "Delete a pool",
                &[
                    format!("Deleting {name} unbinds every member."),
                    String::new(),
                    format!("type its name to confirm: {typed}▏"),
                ],
            );
        }
        (None, None) => {}
    }
}

/// A small centred card: a question and whatever it needs to show first.
fn confirm(frame: &mut Frame, title: &str, lines: &[String]) {
    let height = u16::try_from(lines.len()).unwrap_or(4) + 2;
    let area = frame.area();
    let width = area.width.min(64);
    let card = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, card);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::border_focused())
        .title(format!(" {title} "));
    let inner = block.inner(card);
    frame.render_widget(block, card);
    let text: Vec<Line> = lines.iter().map(|l| Line::from(Span::raw(format!(" {l}")))).collect();
    frame.render_widget(Paragraph::new(text), inner);
}

#[cfg(test)]
mod tests {
    use super::{FIXED_COLS, W_USAGE, name_width};

    #[test]
    fn the_name_column_takes_what_is_left_within_bounds() {
        assert_eq!(name_width(200), 20, "capped so the table does not sprawl");
        assert_eq!(name_width(40), 8, "floored so a name is still readable");
        assert!((8..=20).contains(&name_width(80)));
    }

    #[test]
    fn the_usage_cell_keeps_the_level_and_leaves_the_reset_to_the_detail_pane() {
        const { assert!(W_USAGE >= 8, "`5h 100%` has to fit") };
    }

    #[test]
    fn the_fixed_columns_leave_room_for_a_name_at_eighty() {
        const { assert!(FIXED_COLS + 2 + 8 <= 80, "the floor name width has to fit in 80 columns") };
    }
}
