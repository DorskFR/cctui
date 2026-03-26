use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

const HELP_TEXT: &[(&str, &str)] = &[
    ("q", "Quit application"),
    ("j / ↓", "Select next session"),
    ("k / ↑", "Select previous session"),
    ("Tab", "Switch between tree and detail pane"),
    ("Enter", "Open conversation view"),
    ("Esc", "Go back to sessions view"),
    ("l", "Switch detail to Log mode"),
    ("c", "Switch detail to Conversation mode"),
    ("m", "Open message input"),
    ("?", "Show this help overlay"),
];

pub fn draw(frame: &mut Frame) {
    let area = frame.area();

    // Center a 60x(height) overlay
    let width = area.width.min(60);
    let height = (HELP_TEXT.len() as u16 + 4).min(area.height);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let overlay_area = Rect::new(x, y, width, height);

    frame.render_widget(Clear, overlay_area);

    let block = Block::default()
        .title(" Help — press Esc to close ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(overlay_area);
    frame.render_widget(block, overlay_area);

    let [_title_area, list_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(inner);

    let lines: Vec<Line> = HELP_TEXT
        .iter()
        .map(|(key, desc)| {
            Line::from(vec![
                Span::styled(
                    format!("  {key:<10}"),
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                ),
                Span::styled(*desc, Style::default().fg(Color::White)),
            ])
        })
        .collect();

    frame.render_widget(Paragraph::new(lines), list_area);
}
