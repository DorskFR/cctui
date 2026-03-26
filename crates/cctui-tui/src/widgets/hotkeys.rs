use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

pub struct HotkeysBar;

impl Widget for HotkeysBar {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let key_style = Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD);
        let desc_style = Style::default().fg(Color::Gray);

        let line = Line::from(vec![
            Span::styled("q", key_style),
            Span::styled(":quit ", desc_style),
            Span::styled("j/k", key_style),
            Span::styled(":nav ", desc_style),
            Span::styled("Tab", key_style),
            Span::styled(":pane ", desc_style),
            Span::styled("Enter", key_style),
            Span::styled(":open ", desc_style),
            Span::styled("m", key_style),
            Span::styled(":message ", desc_style),
            Span::styled("?", key_style),
            Span::styled(":help", desc_style),
        ]);
        line.render(area, buf);
    }
}
