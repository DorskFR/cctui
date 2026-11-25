use std::fmt::Write as _;

use cctui_proto::media::is_inline_type;

/// `Content-Disposition` value: `inline` for viewable types, `attachment`
/// otherwise; the filename is quoted with `"` / `\` escaped and non-ASCII
/// carried in the RFC 5987 `filename*` form.
#[must_use]
pub fn content_disposition(media_type: &str, name: &str) -> String {
    let kind = if is_inline_type(media_type) { "inline" } else { "attachment" };
    let ascii: String =
        name.chars()
            .map(|c| {
                if c.is_ascii() && !c.is_ascii_control() && c != '"' && c != '\\' { c } else { '_' }
            })
            .collect();
    let mut value = format!("{kind}; filename=\"{ascii}\"");
    if !name.is_ascii() {
        let _ = write!(value, "; filename*=UTF-8''{}", percent_encode(name));
    }
    value
}

/// RFC 3986 unreserved characters pass, everything else is `%XX`.
#[must_use]
pub fn percent_encode(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
