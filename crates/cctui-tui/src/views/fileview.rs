//! The overlay pager for an agent-linked local file.
//!
//! Text goes through the existing highlighter and `.md` through the existing
//! markdown renderer. An image is drawn inline where the terminal can, and
//! otherwise gets a placeholder chip and `o`, which hands it to the desktop
//! viewer: a terminal that cannot draw images must still get the user to the
//! picture.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::fileview::{FileKind, FileView};
use crate::app::images::Images;
use crate::theme;

pub fn draw(frame: &mut Frame, view: &FileView, area: Rect, images: &Images) {
    frame.render_widget(Clear, area);
    let title = format!(" {} — Esc closes, o opens in the desktop viewer ", view.name);
    let block =
        Block::default().borders(Borders::ALL).border_style(theme::border_focused()).title(title);
    let inner = block.inner(area);

    if view.kind == FileKind::Image {
        frame.render_widget(&block, area);
        if super::images::draw_inline(frame, images, &view.name, &view.bytes, inner) {
            return;
        }
        frame.render_widget(
            Paragraph::new(super::images::placeholder(&view.name, &view.bytes, true)),
            inner,
        );
        return;
    }

    let body = match view.kind {
        FileKind::Image => Vec::new(),
        FileKind::Markdown => crate::ui::markdown_render::render_markdown_text(&view.text).lines,
        // A download never reaches the pager; the effect hands it to the OS.
        FileKind::Text | FileKind::Download => highlighted(view, inner.width as usize),
    };

    let total = body.len();
    let height = inner.height as usize;
    let offset = view.scroll.min(total.saturating_sub(height.min(total)));
    let shown: Vec<Line<'static>> = body.into_iter().skip(offset).take(height).collect();

    frame.render_widget(Paragraph::new(shown).block(block), area);
}

/// Numbered, syntax-highlighted lines for a text file.
fn highlighted(view: &FileView, width: usize) -> Vec<Line<'static>> {
    let gutter = view.text.lines().count().to_string().len().max(3);
    let _ = width;
    crate::ui::highlight::highlight_code_to_lines(&view.text, view.extension())
        .into_iter()
        .enumerate()
        .map(|(i, mut line)| {
            let mut spans = vec![Span::styled(format!("{:>gutter$} ", i + 1), theme::dim())];
            spans.append(&mut line.spans);
            Line::from(spans)
        })
        .collect()
}

