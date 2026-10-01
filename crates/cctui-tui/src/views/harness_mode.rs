//! The harness-mode picker.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::app::harness_mode::{MODES, Picker};
use crate::theme;

pub fn draw(frame: &mut Frame, picker: &Picker) {
    let area = frame.area().inner(Margin { horizontal: 6, vertical: 4 });
    frame.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::border_focused())
        .title(" Harness mode ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [list_area, blurb_area, hotkeys_area] =
        Layout::vertical([Constraint::Length(4), Constraint::Fill(1), Constraint::Length(1)])
            .areas(inner);

    let rows: Vec<Line> = MODES
        .iter()
        .enumerate()
        .map(|(index, mode)| {
            let focused = index == picker.focused;
            let current = *mode == picker.current;
            Line::from(vec![
                Span::styled(if focused { "❯ " } else { "  " }.to_owned(), theme::border_focused()),
                Span::styled(
                    format!("{:<9}", mode.as_str()),
                    if focused { theme::bold() } else { theme::dim() },
                ),
                Span::styled(
                    if current { "in use".to_owned() } else { String::new() },
                    theme::active(),
                ),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(rows), list_area);

    let focused = MODES.get(picker.focused).copied().unwrap_or_default();
    frame.render_widget(
        Paragraph::new(Span::styled(focused.blurb().to_owned(), theme::dim()))
            .wrap(Wrap { trim: true }),
        blurb_area,
    );

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" ↑↓ ", theme::hotkey()),
            Span::raw("Pick  "),
            Span::styled("Enter ", theme::hotkey()),
            Span::raw("Apply to every daemon  "),
            Span::styled("Esc ", theme::hotkey()),
            Span::raw("Cancel"),
        ])),
        hotkeys_area,
    );
}
