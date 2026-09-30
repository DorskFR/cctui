//! The overlay pager for an agent-linked local file.
//!
//! Text goes through the existing highlighter and `.md` through the existing
//! markdown renderer. An image gets a placeholder chip and `o`, which hands it
//! to the desktop viewer: a terminal that cannot draw images must still get the
//! user to the picture.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::fileview::{FileKind, FileView};
use crate::theme;

pub fn draw(frame: &mut Frame, view: &FileView, area: Rect) {
    frame.render_widget(Clear, area);
    let title = format!(" {} — Esc closes, o opens in the desktop viewer ", view.name);
    let block =
        Block::default().borders(Borders::ALL).border_style(theme::border_focused()).title(title);
    let inner = block.inner(area);

    let body = match view.kind {
        FileKind::Image => image_placeholder(view, inner),
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

/// What the pager shows for an image: the inline picture when the terminal can
/// draw one, and the same chip text the composer uses when it cannot.
fn image_placeholder(view: &FileView, area: Rect) -> Vec<Line<'static>> {
    let size = cctui_clientcore::uploads::fmt_size(view.bytes.len() as u64);
    let dims = image_dimensions(&view.bytes).map_or_else(String::new, |(w, h)| format!(" {w}x{h}"));
    let mut lines = vec![
        Line::from(Span::styled(format!("[image: {}{dims} {size}]", view.name), theme::branch())),
        Line::raw(""),
    ];
    let _ = area;
    lines.push(Line::from(Span::styled(
        "press o to open it in the desktop viewer".to_owned(),
        theme::dim(),
    )));
    lines
}

fn image_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}
