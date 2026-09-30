use crate::format::{hash_hue, hue_tint};

pub const LABEL_HUES: [u32; 12] = [0, 30, 60, 90, 120, 150, 180, 210, 240, 270, 300, 330];

/// `parseInt(s, 10)`: a leading integer, trailing junk ignored, `None` when
/// there is no leading digit.
fn js_parse_int(s: &str) -> Option<i64> {
    let t = s.trim_start();
    let mut chars = t.chars().peekable();
    let mut out = String::new();
    if matches!(chars.peek(), Some('+' | '-')) {
        out.push(chars.next().unwrap());
    }
    while let Some(c) = chars.peek() {
        if c.is_ascii_digit() {
            out.push(*c);
            chars.next();
        } else {
            break;
        }
    }
    if out.is_empty() || out == "+" || out == "-" {
        return None;
    }
    out.parse().ok()
}

/// The hue explicitly stored on a label, or `None` when unset ("Auto").
#[must_use]
pub fn stored_hue(color: &str) -> Option<u32> {
    js_parse_int(color).map(|n| (((n % 360) + 360) % 360) as u32)
}

#[must_use]
pub fn label_hue(name: &str, color: &str) -> u32 {
    stored_hue(color).unwrap_or_else(|| hash_hue(name))
}

/// Persisted `color` string for a chosen hue (`None` = Auto/name hash).
#[must_use]
pub fn hue_to_color(hue: Option<u32>) -> String {
    hue.map_or_else(String::new, |h| h.to_string())
}

#[must_use]
pub fn label_tint(name: &str, color: &str) -> String {
    hue_tint(label_hue(name, color))
}
