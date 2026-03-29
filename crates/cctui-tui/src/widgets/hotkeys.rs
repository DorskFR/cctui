use ratatui::Frame;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::theme;

pub fn draw_session_hotkeys(frame: &mut Frame, area: ratatui::layout::Rect) {
    let line = Line::from(vec![
        Span::styled(" j/k", theme::HOTKEY),
        Span::styled(":nav  ", theme::HOTKEY_DESC),
        Span::styled("Enter", theme::HOTKEY),
        Span::styled(":open  ", theme::HOTKEY_DESC),
        Span::styled("g/G", theme::HOTKEY),
        Span::styled(":top/bottom  ", theme::HOTKEY_DESC),
        Span::styled("?", theme::HOTKEY),
        Span::styled(":help  ", theme::HOTKEY_DESC),
        Span::styled("q", theme::HOTKEY),
        Span::styled(":quit", theme::HOTKEY_DESC),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

pub fn draw_conversation_hotkeys(frame: &mut Frame, area: ratatui::layout::Rect) {
    let line = Line::from(vec![
        Span::styled(" j/k", theme::HOTKEY),
        Span::styled(":scroll  ", theme::HOTKEY_DESC),
        Span::styled("g/G", theme::HOTKEY),
        Span::styled(":top/bottom  ", theme::HOTKEY_DESC),
        Span::styled("i", theme::HOTKEY),
        Span::styled(":message  ", theme::HOTKEY_DESC),
        Span::styled("Esc", theme::HOTKEY),
        Span::styled(":back  ", theme::HOTKEY_DESC),
        Span::styled("?", theme::HOTKEY),
        Span::styled(":help", theme::HOTKEY_DESC),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}
