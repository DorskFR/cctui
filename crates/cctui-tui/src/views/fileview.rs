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
        if super::images::draw_inline(frame, images, &view.name, inner) {
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
        FileKind::Text => highlighted(view, inner.width as usize),
        // Nothing here can render it, and handing a session's bytes to the
        // desktop is the user's call to make, not a consequence of opening it.
        FileKind::Download => unknown_type(view),
    };

    let total = body.len();
    let height = inner.height as usize;
    let offset = view.scroll.min(total.saturating_sub(height.min(total)));
    let shown: Vec<Line<'static>> = body.into_iter().skip(offset).take(height).collect();

    frame.render_widget(Paragraph::new(shown).block(block), area);
}

/// What an unopenable file says for itself, plus the key that acts on it.
fn unknown_type(view: &FileView) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled(
            format!(" {} cannot be shown here", view.name),
            theme::section_title(),
        )),
        Line::raw(""),
        Line::from(Span::styled(
            format!(
                "  type: {}",
                if view.content_type.is_empty() { "unknown" } else { view.content_type.trim() }
            ),
            theme::dim(),
        )),
        Line::from(Span::styled(format!("  size: {} bytes", view.bytes.len()), theme::dim())),
        Line::raw(""),
    ];
    match crate::app::fileview::refuse_external_open(&view.name, &view.content_type) {
        Some(why) => lines.push(Line::from(Span::styled(format!("  {why}"), theme::error()))),
        None => lines.push(Line::from(Span::styled(
            "  press o to open it in the desktop viewer".to_owned(),
            theme::hotkey_desc(),
        ))),
    }
    lines
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
