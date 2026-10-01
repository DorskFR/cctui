//! Agent-posted image markers in a transcript body.
//!
//! The daemon rewrites `![alt](/abs/path.png)` into `![alt](cctui-img://<id>)`
//! before it stores a message, so both clients see the same marker: the webui
//! turns it into an `<img>`, the TUI into a placeholder chip it can fetch and
//! page. The scan and the chip wording live here so they cannot drift.

use crate::uploads::fmt_size;

/// One `cctui-img://` marker, with the byte range it occupies in the body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageMarker {
    pub alt: String,
    pub id: String,
    pub start: usize,
    pub end: usize,
}

/// Every marker in `text`, in the order it appears. Ids are server-minted
/// uuids, so anything outside `[A-Za-z0-9-]` is not a marker.
#[must_use]
pub fn scan(text: &str) -> Vec<ImageMarker> {
    const SCHEME: &str = "](cctui-img://";
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(found) = text[at..].find("![") {
        let start = at + found;
        let Some(alt_end) = text[start + 2..].find(']').map(|i| start + 2 + i) else { break };
        at = start + 2;
        if !text[alt_end..].starts_with(SCHEME) {
            continue;
        }
        let id_start = alt_end + SCHEME.len();
        let Some(close) = text[id_start..].find(')').map(|i| id_start + i) else { continue };
        let id = &text[id_start..close];
        if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            continue;
        }
        if bytes[start + 2..alt_end].contains(&b'[') {
            continue;
        }
        out.push(ImageMarker {
            alt: text[start + 2..alt_end].to_owned(),
            id: id.to_owned(),
            start,
            end: close + 1,
        });
        at = close + 1;
    }
    out
}

/// The chip wording, without its brackets: what the composer shows for a
/// staged image, and what the transcript shows for a posted one.
#[must_use]
pub fn placeholder_label(name: &str, dimensions: Option<(u32, u32)>, bytes: Option<u64>) -> String {
    let name = name.trim();
    let mut out = if name.is_empty() { "image".to_owned() } else { format!("image: {name}") };
    if let Some((w, h)) = dimensions {
        use std::fmt::Write as _;
        let _ = write!(out, " {w}x{h}");
    }
    if let Some(size) = bytes {
        out.push(' ');
        out.push_str(&fmt_size(size));
    }
    out
}

/// `[image: shot.png]` — the bracketed form that stands in for a marker in a
/// body of prose.
#[must_use]
pub fn inline_placeholder(
    name: &str,
    dimensions: Option<(u32, u32)>,
    bytes: Option<u64>,
) -> String {
    format!("[{}]", placeholder_label(name, dimensions, bytes))
}

/// `text` with every marker replaced by its placeholder. Nothing else about the
/// body changes, so a line of prose around an image keeps reading as prose.
#[must_use]
pub fn substitute(text: &str) -> String {
    let markers = scan(text);
    if markers.is_empty() {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for marker in &markers {
        out.push_str(&text[at..marker.start]);
        out.push_str(&inline_placeholder(&marker.alt, None, None));
        at = marker.end;
    }
    out.push_str(&text[at..]);
    out
}

/// Whether the body is nothing but images: such a message is a picture, not a
/// sentence with a picture in it, and gets a line of its own.
#[must_use]
pub fn is_only_images(text: &str) -> bool {
    let markers = scan(text);
    if markers.is_empty() {
        return false;
    }
    let mut rest = String::new();
    let mut at = 0;
    for marker in &markers {
        rest.push_str(&text[at..marker.start]);
        at = marker.end;
    }
    rest.push_str(&text[at..]);
    rest.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::{inline_placeholder, is_only_images, placeholder_label, scan, substitute};

    #[test]
    fn a_marker_yields_its_alt_and_id() {
        let found = scan("before ![shot.png](cctui-img://ab-12) after");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].alt, "shot.png");
        assert_eq!(found[0].id, "ab-12");
        assert_eq!(
            &"before ![shot.png](cctui-img://ab-12) after"[found[0].start..found[0].end],
            "![shot.png](cctui-img://ab-12)"
        );
    }

    #[test]
    fn a_foreign_url_is_not_a_marker() {
        assert!(scan("![x](https://evil.example/x.png)").is_empty());
        assert!(scan("![x](cctui-img://bad id)").is_empty());
        assert!(scan("![x](cctui-img://)").is_empty());
    }

    #[test]
    fn substitution_keeps_the_prose_around_the_picture() {
        assert_eq!(
            substitute("see ![a.png](cctui-img://1) and ![](cctui-img://2)"),
            "see [image: a.png] and [image]"
        );
        assert_eq!(substitute("no images"), "no images");
    }

    #[test]
    fn a_body_of_only_markers_is_an_image_message() {
        assert!(is_only_images("![a.png](cctui-img://1)\n![b.png](cctui-img://2)"));
        assert!(!is_only_images("here it is ![a.png](cctui-img://1)"));
        assert!(!is_only_images("plain"));
    }

    #[test]
    fn the_chip_grows_with_what_is_known() {
        assert_eq!(placeholder_label("shot.png", None, None), "image: shot.png");
        assert_eq!(
            placeholder_label("shot.png", Some((1280, 720)), Some(348_160)),
            "image: shot.png 1280x720 340 KB"
        );
        assert_eq!(placeholder_label("  ", None, Some(12)), "image 12 B");
        assert_eq!(inline_placeholder("a.png", None, None), "[image: a.png]");
    }
}
