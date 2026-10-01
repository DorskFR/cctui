//! The `f` popup: which sections the list shows.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::App;
use crate::app::list_view::SECTIONS;
use crate::theme;

const WIDTH: u16 = 24;

/// Centred, and clipped rather than drawn outside a short frame.
fn area(frame_area: Rect, rows: u16) -> Rect {
    let height = (rows + 2).min(frame_area.height);
    let width = WIDTH.min(frame_area.width);
    Rect {
        x: frame_area.x + (frame_area.width.saturating_sub(width)) / 2,
        y: frame_area.y + (frame_area.height.saturating_sub(height)) / 2,
        width,
        height,
    }
}

pub fn draw(frame: &mut Frame, app: &App) {
    let Some(focused) = app.sections_menu else { return };
    let rows = u16::try_from(SECTIONS.len() + 1).unwrap_or(u16::MAX);
    let area = area(frame.area(), rows);
    frame.render_widget(Clear, area);

    let mut lines: Vec<Line> = Vec::with_capacity(SECTIONS.len() + 1);
    for (index, section) in SECTIONS.iter().enumerate() {
        let on = app.list_shape.sections.has(*section);
        let style = if index == focused {
            theme::selected()
        } else if on {
            theme::bold()
        } else {
            theme::dim()
        };
        lines.push(Line::from(vec![
            Span::styled(if index == focused { " ❯ " } else { "   " }, style),
            Span::styled(format!("[{}] {}", if on { 'x' } else { ' ' }, section.title()), style),
        ]));
    }
    lines.push(Line::from(Span::styled("   space toggle  esc", theme::dim())));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::border_focused())
        .title(" Sections ");
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

#[cfg(test)]
mod tests {
    use ratatui::layout::Rect;

    #[test]
    fn the_popup_is_centred_and_clipped_to_the_frame() {
        let frame = Rect { x: 0, y: 0, width: 100, height: 24 };
        let area = super::area(frame, 7);
        assert_eq!(area.width, super::WIDTH);
        assert_eq!(area.height, 9);
        assert_eq!(area.x, (100 - super::WIDTH) / 2);

        let tiny = Rect { x: 0, y: 0, width: 10, height: 4 };
        let clipped = super::area(tiny, 7);
        assert_eq!(clipped.width, 10);
        assert_eq!(clipped.height, 4);
    }
}
