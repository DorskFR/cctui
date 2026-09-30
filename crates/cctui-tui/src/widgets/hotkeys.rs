use ratatui::Frame;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::theme;

pub fn draw_session_hotkeys(frame: &mut Frame, area: ratatui::layout::Rect) {
    let line = Line::from(vec![
        Span::styled(" j/k", theme::hotkey()),
        Span::styled(":nav  ", theme::hotkey_desc()),
        Span::styled("Enter", theme::hotkey()),
        Span::styled(":open  ", theme::hotkey_desc()),
        Span::styled("g/G", theme::hotkey()),
        Span::styled(":top/bottom  ", theme::hotkey_desc()),
        Span::styled("?", theme::hotkey()),
        Span::styled(":help  ", theme::hotkey_desc()),
        Span::styled("q", theme::hotkey()),
        Span::styled(":quit", theme::hotkey_desc()),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}
