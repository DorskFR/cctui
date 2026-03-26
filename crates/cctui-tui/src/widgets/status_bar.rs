use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

pub struct StatusBar {
    pub session_count: usize,
    pub active_count: usize,
    pub total_tokens: u64,
    pub total_cost: f64,
}

impl StatusBar {
    pub const fn new(
        session_count: usize,
        active_count: usize,
        total_tokens: u64,
        total_cost: f64,
    ) -> Self {
        Self { session_count, active_count, total_tokens, total_cost }
    }
}

impl Widget for StatusBar {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let label_style = Style::default().fg(Color::DarkGray);
        let value_style = Style::default().fg(Color::White).add_modifier(Modifier::BOLD);
        let active_style = Style::default().fg(Color::Green).add_modifier(Modifier::BOLD);

        let line = Line::from(vec![
            Span::styled(" Sessions: ", label_style),
            Span::styled(self.session_count.to_string(), value_style),
            Span::styled("  Active: ", label_style),
            Span::styled(self.active_count.to_string(), active_style),
            Span::styled("  Tokens: ", label_style),
            Span::styled(self.total_tokens.to_string(), value_style),
            Span::styled("  Cost: ", label_style),
            Span::styled(format!("${:.4}", self.total_cost), Style::default().fg(Color::Yellow)),
        ]);
        line.render(area, buf);
    }
}
