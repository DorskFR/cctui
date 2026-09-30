//! The `F` menu: one row per transcript category, over the conversation.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::App;
use crate::app::transcript_filter::CATEGORIES;
use crate::theme;

const WIDTH: u16 = 38;

/// Centred, and never taller than the frame: the list is fixed-length, so a
/// short terminal clips rows rather than drawing outside its area.
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
    let Some(focused) = app.filter_menu else { return };
    let rows = u16::try_from(CATEGORIES.len() + 1).unwrap_or(u16::MAX);
    let area = area(frame.area(), rows);
    frame.render_widget(Clear, area);

    let mut lines: Vec<Line> = Vec::with_capacity(CATEGORIES.len() + 1);
    for (index, category) in CATEGORIES.iter().enumerate() {
        let shown = app.filter.shows(*category);
        let mark = if shown { "[x]" } else { "[ ]" };
        let style = if index == focused {
            theme::selected()
        } else if shown {
            theme::bold()
        } else {
            theme::dim()
        };
        lines.push(Line::from(vec![
            Span::styled(if index == focused { " ❯ " } else { "   " }, style),
            Span::styled(format!("{mark} {}", category.title()), style),
        ]));
    }
    lines.push(Line::from(Span::styled("   space · a all · r reset · F close", theme::dim())));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::border_focused())
        .title(" show which lines ");
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

#[cfg(test)]
mod tests {
    use ratatui::layout::Rect;

    #[test]
    fn the_menu_is_centred_and_clipped_to_the_frame() {
        let frame = Rect { x: 0, y: 0, width: 100, height: 24 };
        let area = super::area(frame, 16);
        assert_eq!(area.width, super::WIDTH);
        assert_eq!(area.height, 18);
        assert_eq!(area.x, (100 - super::WIDTH) / 2);

        let tiny = Rect { x: 0, y: 0, width: 20, height: 6 };
        let clipped = super::area(tiny, 16);
        assert_eq!(clipped.width, 20, "a narrow frame caps the width");
        assert_eq!(clipped.height, 6, "a short frame caps the height");
    }
}
