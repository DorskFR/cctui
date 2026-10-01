//! The one modal list overlay: prompt history, pins, macros.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::theme;

/// One row: an optional dim lead (a seq, a target) before the text.
pub struct PickerRow {
    pub lead: String,
    pub text: String,
}

impl PickerRow {
    pub fn plain(text: impl Into<String>) -> Self {
        Self { lead: String::new(), text: text.into() }
    }
}

pub struct Picker<'a> {
    pub title: &'a str,
    /// `None` leaves the input line out, for a list that is not filterable.
    pub filter: Option<&'a str>,
    /// What the input line is for: a list narrows, a prompt names something.
    pub prompt: &'a str,
    pub rows: &'a [PickerRow],
    pub selected: usize,
    pub empty: &'a str,
    /// Key/label pairs for the footer.
    pub hotkeys: &'a [(&'a str, &'a str)],
}

pub fn draw(frame: &mut Frame, picker: &Picker) {
    let area = frame.area().inner(Margin { horizontal: 6, vertical: 4 });
    frame.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::border_focused())
        .title(format!(" {} ", picker.title));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let filter_height = u16::from(picker.filter.is_some()) * 2;
    let [filter_area, list_area, hotkeys_area] = Layout::vertical([
        Constraint::Length(filter_height),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(inner);

    if let Some(filter) = picker.filter {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!(" {} ", picker.prompt), theme::dim()),
                Span::raw(filter.to_owned()),
                Span::styled("▏", theme::border_focused()),
            ])),
            filter_area,
        );
    }

    if picker.rows.is_empty() {
        frame.render_widget(Paragraph::new(Span::styled(picker.empty, theme::dim())), list_area);
    } else {
        let height = list_area.height as usize;
        let first = picker.selected.saturating_sub(height.saturating_sub(1));
        let rows: Vec<Line> = picker
            .rows
            .iter()
            .enumerate()
            .skip(first)
            .take(height)
            .map(|(index, row)| render_row(index == picker.selected, row))
            .collect();
        frame.render_widget(Paragraph::new(rows), list_area);
    }

    let mut footer = Vec::new();
    for (keys, label) in picker.hotkeys {
        footer.push(Span::styled(format!(" {keys} "), theme::hotkey()));
        footer.push(Span::raw(format!("{label} ")));
    }
    frame.render_widget(Paragraph::new(Line::from(footer)), hotkeys_area);
}

/// Every row is one screen line whatever its content: newlines become `⏎` so a
/// pasted block cannot push the rest of the list off screen.
fn render_row(selected: bool, row: &PickerRow) -> Line<'static> {
    let text = row.text.replace('\n', " ⏎ ");
    let mut spans = vec![Span::styled(
        if selected { "❯ " } else { "  " },
        if selected { theme::border_focused() } else { theme::dim() },
    )];
    if !row.lead.is_empty() {
        spans.push(Span::styled(format!("{} ", row.lead), theme::dim()));
    }
    spans.push(Span::styled(text, if selected { theme::bold() } else { theme::dim() }));
    Line::from(spans)
}
