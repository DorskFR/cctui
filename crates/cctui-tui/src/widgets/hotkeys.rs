use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

const BINDINGS: &[(&str, &str)] = &[
    ("q", "quit"),
    ("j/k", "nav"),
    ("Tab", "pane"),
    ("Enter", "open"),
    ("Esc", "back"),
    ("m", "msg"),
    ("l/c", "log/conv"),
    ("?", "help"),
];

pub struct HotkeysBar;

impl Widget for HotkeysBar {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let key_style = Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD);
        let sep_style = Style::default().fg(Color::DarkGray);

        let mut spans: Vec<Span> = Vec::with_capacity(BINDINGS.len() * 3);
        for (i, (key, desc)) in BINDINGS.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled("  ", sep_style));
            }
            spans.push(Span::styled(*key, key_style));
            spans.push(Span::styled(format!(":{desc}"), sep_style));
        }

        Line::from(spans).render(area, buf);
    }
}
