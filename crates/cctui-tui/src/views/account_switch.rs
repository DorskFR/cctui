//! The session account picker.

use cctui_clientcore::account_switch::Option_;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::account_switch::Picker;
use crate::theme;

fn usage_cell(option: &Option_) -> String {
    if option.limited {
        return "limited".to_owned();
    }
    option.pct.map_or_else(|| "—".to_owned(), |pct| format!("{}%", pct.round() as i64))
}

fn reset_cell(option: &Option_) -> String {
    match option.resets_in_secs {
        Some(secs) if secs > 0 => {
            let mins = secs / 60;
            if mins >= 60 { format!("{}h", mins / 60) } else { format!("{mins}m") }
        }
        _ => String::new(),
    }
}

fn row(option: &Option_, focused: bool, recommended: bool) -> Line<'static> {
    let mut spans = vec![
        Span::styled(if focused { "❯ " } else { "  " }.to_owned(), theme::border_focused()),
        Span::styled(
            format!("{:<16}", option.account_name),
            if focused { theme::bold() } else { theme::dim() },
        ),
        Span::styled(format!("{:<9}", usage_cell(option)), {
            if option.limited { theme::error() } else { theme::dim() }
        }),
        Span::styled(format!("{:<5}", reset_cell(option)), theme::dim()),
    ];
    if option.current {
        spans.push(Span::styled("in use".to_owned(), theme::active()));
    } else if recommended {
        spans.push(Span::styled("recommended".to_owned(), theme::active()));
    }
    Line::from(spans)
}

pub fn draw(frame: &mut Frame, picker: &Picker) {
    let area = frame.area().inner(Margin { horizontal: 6, vertical: 4 });
    frame.render_widget(Clear, area);

    let title = picker.binding().map_or_else(
        || " Switch account ".to_owned(),
        |b| format!(" Switch account · {} ", b.family),
    );
    let block =
        Block::default().borders(Borders::ALL).border_style(theme::border_focused()).title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [list_area, note_area, hotkeys_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1), Constraint::Length(1)])
            .areas(inner);

    let best = cctui_clientcore::account_switch::recommended(&picker.options);
    let rows: Vec<Line> = if picker.loading {
        vec![Line::from(Span::styled("loading…".to_owned(), theme::dim()))]
    } else if picker.options.is_empty() {
        vec![Line::from(Span::styled(
            "this session has no switchable account binding".to_owned(),
            theme::dim(),
        ))]
    } else {
        picker
            .options
            .iter()
            .enumerate()
            .map(|(index, option)| row(option, index == picker.focused, best == Some(index)))
            .collect()
    };
    frame.render_widget(Paragraph::new(rows), list_area);

    let note = if picker.switching {
        Span::styled("switching…".to_owned(), theme::dim())
    } else if let Some(error) = picker.error.as_ref() {
        Span::styled(error.clone(), theme::error())
    } else if picker.bindings.len() > 1 {
        Span::styled(
            format!("binding {} of {}", picker.binding_ix + 1, picker.bindings.len()),
            theme::dim(),
        )
    } else {
        Span::raw("")
    };
    frame.render_widget(Paragraph::new(Line::from(note)), note_area);

    let mut hotkeys = vec![
        Span::styled(" ↑↓ ", theme::hotkey()),
        Span::raw("Pick  "),
        Span::styled("Enter ", theme::hotkey()),
        Span::raw("Switch  "),
    ];
    if picker.bindings.len() > 1 {
        hotkeys.push(Span::styled("tab ", theme::hotkey()));
        hotkeys.push(Span::raw("Family  "));
    }
    hotkeys.push(Span::styled("Esc ", theme::hotkey()));
    hotkeys.push(Span::raw("Cancel"));
    frame.render_widget(Paragraph::new(Line::from(hotkeys)), hotkeys_area);
}
