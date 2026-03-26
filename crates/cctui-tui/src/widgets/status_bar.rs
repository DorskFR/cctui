use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

pub struct StatusBar {
    pub session_count: usize,
    pub active_count: usize,
    pub total_tokens: u64,
    pub total_cost: f64,
}

impl Widget for StatusBar {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let text = Line::from(vec![
            Span::styled("Sessions: ", Style::default().fg(Color::Gray)),
            Span::raw(format!("{}", self.session_count)),
            Span::raw("  "),
            Span::styled("Active: ", Style::default().fg(Color::Green)),
            Span::raw(format!("{}", self.active_count)),
            Span::raw("  "),
            Span::styled("Tokens: ", Style::default().fg(Color::Blue)),
            Span::raw(format!("{}", self.total_tokens)),
            Span::raw("  "),
            Span::styled("Cost: ", Style::default().fg(Color::Yellow)),
            Span::raw(format!("${:.4}", self.total_cost)),
        ]);
        text.render(area, buf);
    }
}
