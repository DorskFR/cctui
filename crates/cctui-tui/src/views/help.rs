use ratatui::Frame;
use ratatui::layout::Margin;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::theme;

const BINDINGS: &[(&str, &str)] = &[
    ("q", "Quit"),
    ("j / ↓", "Next session / scroll down"),
    ("k / ↑", "Previous session / scroll up"),
    ("g / G", "First / last"),
    ("Enter", "Open conversation"),
    ("Esc", "Back to sessions / exit input"),
    ("type", "Start composing message"),
    ("Shift+Enter", "New line in message"),
    ("t", "Toggle timestamps"),
    ("f", "Toggle tool details"),
    ("?", "Toggle help"),
];

pub fn draw(frame: &mut Frame) {
    let area = frame.area().inner(Margin { horizontal: 10, vertical: 4 });
    frame.render_widget(Clear, area);

    let block =
        Block::default().borders(Borders::ALL).border_style(theme::BORDER_FOCUSED).title(" Help ");

    let lines: Vec<Line> = BINDINGS
        .iter()
        .map(|(key, desc)| {
            Line::from(vec![Span::styled(format!("  {key:<12}"), theme::HOTKEY), Span::raw(*desc)])
        })
        .collect();

    frame.render_widget(Paragraph::new(lines).block(block), area);
}
