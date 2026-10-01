//! The dispatchers panel and its dialogs.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};

use crate::app::App;
use crate::app::dispatchers::{Field, Form, Mode, binding_text, can_manage, state_text};
use crate::theme;

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area().inner(Margin { horizontal: 4, vertical: 2 });
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::border_focused())
        .title(" Dispatchers ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [list_area, hotkeys_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(inner);

    match app.dispatchers.mode() {
        Mode::Enroll { form, field } => draw_form(frame, list_area, "Enroll", &form, field),
        Mode::Edit { form, field, .. } => draw_form(frame, list_area, "Edit", &form, field),
        Mode::ConfirmDelete { name, .. } => frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" remove \"{name}\"? Enter confirms, Esc keeps it"),
                theme::error(),
            ))),
            list_area,
        ),
        Mode::ShowKey { name, key } => draw_key(frame, list_area, &name, &key),
        Mode::Browse => draw_list(frame, app, list_area),
    }
    frame.render_widget(Paragraph::new(hotkeys(app)), hotkeys_area);
}

fn draw_list(frame: &mut Frame, app: &App, area: Rect) {
    if let Some(error) = &app.dispatchers.error {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(format!(" {error}"), theme::error()))),
            area,
        );
        return;
    }
    if app.dispatchers.rows.is_empty() {
        let text = if app.dispatchers.loading { " loading…" } else { " none enrolled" };
        frame.render_widget(Paragraph::new(Line::from(Span::styled(text, theme::dim()))), area);
        return;
    }

    let name_width = 16;
    let header = Line::from(Span::styled(
        format!("   {:<name_width$}{:<12}{:<11}{}", "NAME", "KIND", "STATE", "DEFAULT"),
        theme::dim(),
    ));
    let [header_area, rows_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);
    frame.render_widget(Paragraph::new(header), header_area);

    let items: Vec<ListItem> = app
        .dispatchers
        .rows
        .iter()
        .map(|row| {
            let name = crate::app::session_status::truncate(&row.name, name_width - 1);
            ListItem::new(Line::from(vec![
                Span::styled(format!(" {name:<name_width$}"), theme::bold()),
                Span::styled(format!("{:<12}", row.kind), theme::dim()),
                Span::styled(format!("{:<11}", state_text(row)), state_tint(row)),
                Span::styled(binding_text(row), theme::branch()),
            ]))
        })
        .collect();
    let list = List::new(items).highlight_style(theme::selected()).highlight_symbol("▸ ");
    let mut state = ListState::default().with_selected(Some(app.dispatchers.selected));
    frame.render_stateful_widget(list, rows_area, &mut state);
}

fn state_tint(row: &cctui_client::Dispatcher) -> ratatui::style::Style {
    if row.connected {
        return theme::active();
    }
    match row.liveness {
        cctui_proto::models::MachineLiveness::Online => theme::active(),
        cctui_proto::models::MachineLiveness::Stale => theme::stale(),
        cctui_proto::models::MachineLiveness::Offline => theme::error(),
    }
}

fn draw_form(frame: &mut Frame, area: Rect, title: &str, form: &Form, field: Field) {
    let row = |label: Field, value: &str| {
        let focused = label == field;
        let marker = if focused { "❯ " } else { "  " };
        Line::from(vec![
            Span::styled(marker, theme::border_focused()),
            Span::styled(format!("{:<22}", label.label()), theme::dim()),
            Span::raw(value.to_owned()),
            Span::styled(if focused { "▏" } else { "" }, theme::dim()),
        ])
    };
    let lines = vec![
        Line::from(Span::styled(format!(" {title} a dispatcher"), theme::section_title())),
        Line::raw(""),
        row(Field::Name, &form.name),
        row(Field::Kind, form.kind()),
        row(Field::Binding, &form.binding),
        Line::raw(""),
        Line::from(Span::styled(
            "  a binding of `pool:<name>` elects within a pool; anything else is an account",
            theme::dim(),
        )),
    ];
    frame.render_widget(Paragraph::new(lines), area);
}

/// The one time the key is on screen. It is not in any log, toast or draft, and
/// it is gone as soon as this dialog closes — the server kept only a hash.
fn draw_key(frame: &mut Frame, area: Rect, name: &str, key: &str) {
    let lines = vec![
        Line::from(Span::styled(format!(" {name} is enrolled"), theme::section_title())),
        Line::raw(""),
        Line::from(Span::styled(
            "  its key is shown once and cannot be fetched again:",
            theme::dim(),
        )),
        Line::raw(""),
        Line::from(Span::styled(format!("  {key}"), theme::bold())),
        Line::raw(""),
        Line::from(Span::styled(
            "  Ctrl+y copies it, Enter closes. Give it to the dispatcher now.",
            theme::error(),
        )),
    ];
    frame.render_widget(Paragraph::new(lines), area);
}

fn hotkeys(app: &App) -> Line<'static> {
    let pair = |key: &'static str, desc: &'static str| {
        [Span::styled(key, theme::hotkey()), Span::styled(desc, theme::hotkey_desc())]
    };
    let mut spans: Vec<Span<'static>> = Vec::new();
    match app.dispatchers.mode() {
        Mode::Browse => {
            spans.extend(pair(" ↑/↓", ":move  "));
            if can_manage(app) {
                spans.extend(pair("Ctrl+a", ":enroll  "));
                spans.extend(pair("Ctrl+e", ":edit  "));
                spans.extend(pair("Ctrl+x", ":remove  "));
            } else {
                spans.push(Span::styled("read-only (no enroll scope)  ", theme::dim()));
            }
            spans.extend(pair("Ctrl+r", ":refresh  "));
            spans.extend(pair("Esc", ":close"));
        }
        Mode::Enroll { .. } | Mode::Edit { .. } => {
            spans.extend(pair(" Tab", ":field  "));
            spans.extend(pair("Enter", ":save  "));
            spans.extend(pair("Esc", ":back"));
        }
        Mode::ConfirmDelete { .. } => {
            spans.extend(pair(" Enter", ":remove  "));
            spans.extend(pair("Esc", ":keep"));
        }
        Mode::ShowKey { .. } => {
            spans.extend(pair(" Ctrl+y", ":copy  "));
            spans.extend(pair("Enter", ":close"));
        }
    }
    Line::from(spans)
}
