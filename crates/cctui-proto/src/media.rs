//! Media-type sniffing for files read off a machine (`ReadFile`).
//!
//! Magic bytes first, then a UTF-8 text check with a markdown/plain split by
//! extension, else `application/octet-stream`. Never yields an active type
//! (html, svg, xml) so the server can serve the result inline with `nosniff`.

use std::path::Path;

pub const OCTET_STREAM: &str = "application/octet-stream";

/// Sniff `bytes` (the first few KiB suffice) for the file called `name`.
#[must_use]
pub fn sniff_media_type(name: &str, bytes: &[u8]) -> &'static str {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return "image/png";
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return "image/jpeg";
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return "image/gif";
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return "image/webp";
    }
    if bytes.starts_with(b"%PDF-") {
        return "application/pdf";
    }
    if is_text(bytes) {
        let ext = Path::new(name).extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
        return match ext.as_deref() {
            Some("md" | "markdown") => "text/markdown; charset=utf-8",
            Some("json") => "application/json; charset=utf-8",
            _ => "text/plain; charset=utf-8",
        };
    }
    OCTET_STREAM
}

/// Whether the (possibly truncated) sample decodes as UTF-8 with no NUL byte.
/// A cut in the middle of a multi-byte sequence at the very end is tolerated.
fn is_text(bytes: &[u8]) -> bool {
    if bytes.contains(&0) {
        return false;
    }
    match std::str::from_utf8(bytes) {
        Ok(_) => true,
        Err(e) => e.error_len().is_none() && bytes.len() - e.valid_up_to() < 4,
    }
}

/// Whether a type is safe and useful to show inline in a browser tab
/// (images, text, markdown, pdf); everything else is served as an attachment.
#[must_use]
pub fn is_inline_type(media_type: &str) -> bool {
    let base = media_type.split(';').next().unwrap_or("").trim();
    matches!(
        base,
        "image/png"
            | "image/jpeg"
            | "image/gif"
            | "image/webp"
            | "text/plain"
            | "text/markdown"
            | "application/json"
            | "application/pdf"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_images_and_pdf_by_magic() {
        assert_eq!(
            sniff_media_type("x.txt", &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]),
            "image/png"
        );
        assert_eq!(sniff_media_type("x", &[0xFF, 0xD8, 0xFF, 0xE0]), "image/jpeg");
        assert_eq!(sniff_media_type("x", b"GIF89a...."), "image/gif");
        let mut webp = b"RIFF\0\0\0\0WEBPVP8 ".to_vec();
        webp.extend_from_slice(&[0; 8]);
        assert_eq!(sniff_media_type("x", &webp), "image/webp");
        assert_eq!(sniff_media_type("x.md", b"%PDF-1.7\n"), "application/pdf");
    }

    #[test]
    fn text_splits_by_extension_and_binary_is_octet() {
        assert_eq!(sniff_media_type("report.md", b"# hi\n"), "text/markdown; charset=utf-8");
        assert_eq!(sniff_media_type("a.json", b"{}"), "application/json; charset=utf-8");
        assert_eq!(
            sniff_media_type("index.html", b"<script>x</script>"),
            "text/plain; charset=utf-8"
        );
        assert_eq!(sniff_media_type("logo.svg", b"<svg/>"), "text/plain; charset=utf-8");
        assert_eq!(sniff_media_type("a.bin", &[0, 1, 2]), OCTET_STREAM);
        assert_eq!(sniff_media_type("a.txt", &[0xFF, 0xFE, 0x41]), OCTET_STREAM);
    }

    #[test]
    fn truncated_utf8_tail_is_still_text() {
        let s = "héllo wörld".as_bytes();
        let cut = &s[..s.len() - 1];
        assert_eq!(sniff_media_type("a.txt", cut), "text/plain; charset=utf-8");
    }

    #[test]
    fn inline_types() {
        assert!(is_inline_type("text/markdown; charset=utf-8"));
        assert!(is_inline_type("image/png"));
        assert!(!is_inline_type("text/html"));
        assert!(!is_inline_type(OCTET_STREAM));
    }
}
