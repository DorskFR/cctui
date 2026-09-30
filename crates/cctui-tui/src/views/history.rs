use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::drafts::Picker;
use crate::theme;

pub fn draw(frame: &mut Frame, picker: &Picker) {
    let area = frame.area().inner(Margin { horizontal: 6, vertical: 4 });
    frame.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::border_focused())
        .title(" Prompt history ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [filter_area, list_area, hotkeys_area] =
        Layout::vertical([Constraint::Length(2), Constraint::Fill(1), Constraint::Length(1)])
            .areas(inner);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" filter ", theme::dim()),
            Span::raw(picker.filter.clone()),
            Span::styled("▏", theme::border_focused()),
        ])),
        filter_area,
    );

    let matches = picker.matches();
    if matches.is_empty() {
        let text = if picker.filter.is_empty() { " no prompts yet" } else { " no match" };
        frame.render_widget(Paragraph::new(Span::styled(text, theme::dim())), list_area);
    } else {
        let height = list_area.height as usize;
        let first = picker.selected.saturating_sub(height.saturating_sub(1));
        let rows: Vec<Line> = matches
            .iter()
            .enumerate()
            .skip(first)
            .take(height)
            .map(|(index, entry)| row(index == picker.selected, entry))
            .collect();
        frame.render_widget(Paragraph::new(rows), list_area);
    }

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" ↑↓ ", theme::hotkey()),
            Span::raw("Pick  "),
            Span::styled("Enter ", theme::hotkey()),
            Span::raw("Recall  "),
            Span::styled("Esc ", theme::hotkey()),
            Span::raw("Cancel"),
        ])),
        hotkeys_area,
    );
}

/// A prompt is one row whatever its length: newlines become `⏎` so a pasted
/// block cannot push the rest of the list off screen.
fn row(selected: bool, entry: &str) -> Line<'static> {
    let text = entry.replace('\n', " ⏎ ");
    if selected {
        Line::from(vec![
            Span::styled("❯ ", theme::border_focused()),
            Span::styled(text, theme::bold()),
        ])
    } else {
        Line::from(Span::styled(format!("  {text}"), theme::dim()))
    }
}
