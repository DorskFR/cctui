use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::mentions::{MentionPopup, VISIBLE_ROWS};
use crate::theme;

/// The completion sits directly above the composer and never taller than it
/// has rows to show, so the transcript keeps as much room as possible.
pub fn draw(frame: &mut Frame, popup: &MentionPopup, input_area: Rect) {
    let rows = popup.matches.len().min(VISIBLE_ROWS);
    let height = u16::try_from(rows).unwrap_or(u16::MAX).saturating_add(2);
    let width = input_area.width.min(60);
    if height > input_area.y || width < 8 {
        return;
    }
    let area = Rect { x: input_area.x, y: input_area.y - height, width, height };
    frame.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::border_focused())
        .title(" #session ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let first = popup.selected.saturating_sub(rows.saturating_sub(1));
    let lines: Vec<Line> = popup
        .matches
        .iter()
        .enumerate()
        .skip(first)
        .take(rows)
        .map(|(index, session)| {
            let selected = index == popup.selected;
            let detail = MentionPopup::row_detail(session);
            let mut spans = vec![
                Span::styled(
                    if selected { "❯ " } else { "  " },
                    if selected { theme::border_focused() } else { theme::dim() },
                ),
                Span::styled(
                    MentionPopup::row_label(session),
                    if selected { theme::bold() } else { theme::dim() },
                ),
            ];
            if !detail.is_empty() {
                spans.push(Span::styled(format!("  {detail}"), theme::dim()));
            }
            Line::from(spans)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}
