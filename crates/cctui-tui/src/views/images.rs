//! Drawing a picture, or saying what it is.
//!
//! Every terminal gets the placeholder; only one that answered a graphics query
//! gets the picture, and the placeholder is still what a snapshot records.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui_image::Image;

use crate::app::images::{Images, dimensions};
use crate::app::state::ConversationLine;
use crate::theme;

/// The chip and the hint that stand in for an image, used by the pager and by
/// the transcript's own image line.
#[must_use]
pub fn placeholder(name: &str, bytes: &[u8], hint: bool) -> Vec<Line<'static>> {
    let size = (!bytes.is_empty()).then_some(bytes.len() as u64);
    let label = cctui_clientcore::images::placeholder_label(name, dimensions(bytes), size);
    let mut lines = vec![Line::from(Span::styled(format!("[{label}]"), theme::branch()))];
    if hint {
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            "o opens it in the desktop viewer, i toggles the inline preview".to_owned(),
            theme::dim(),
        )));
    }
    lines
}

/// Draw the picture itself. `false` means the caller must fall back to the
/// placeholder: no protocol, an undecodable blob, or too little room.
pub fn draw_inline(
    frame: &mut Frame,
    images: &Images,
    name: &str,
    bytes: &[u8],
    area: Rect,
) -> bool {
    let Some(protocol) = images.encode(name, bytes, area) else { return false };
    frame.render_widget(Image::new(&protocol), area);
    true
}

/// A transcript line that is nothing but images.
#[must_use]
pub fn transcript_lines(line: &ConversationLine, ts: &str) -> Vec<Line<'static>> {
    let mut spans = vec![Span::raw(ts.to_owned())];
    spans.push(Span::styled(crate::app::images::line_placeholder(line), theme::branch()));
    if !line.image_ids.is_empty() {
        spans.push(Span::styled(" · g i opens it".to_owned(), theme::dim()));
    }
    vec![Line::from(""), Line::from(spans)]
}

#[cfg(test)]
mod tests {
    use super::{placeholder, transcript_lines};
    use crate::app::state::{ConversationLine, LineKind};

    fn text(lines: &[ratatui::text::Line<'static>]) -> String {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn an_undecodable_blob_still_gets_a_chip_with_its_size() {
        let body = placeholder("shot.png", &[0; 12], false);
        assert_eq!(text(&body), "[image: shot.png 12 B]");
    }

    #[test]
    fn the_pager_chip_says_how_to_see_the_picture() {
        let body = placeholder("shot.png", &[0; 12], true);
        assert!(text(&body).contains("desktop viewer"));
        assert!(text(&body).contains("inline preview"));
    }

    #[test]
    fn a_transcript_image_line_names_the_key_that_opens_it() {
        let mut line = ConversationLine::new(LineKind::Image, "[image: diagram.png]", 0);
        line.image_ids = vec!["img-1".to_owned()];
        let body = transcript_lines(&line, "12:00 ");
        assert_eq!(text(&body), "\n12:00 [image: diagram.png] · g i opens it");
    }
}
