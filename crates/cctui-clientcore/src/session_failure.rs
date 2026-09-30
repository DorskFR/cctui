use crate::uri::encode_uri_component;

pub const FAILED_START_REASONS: [&str; 2] = ["resume_failed", "spawn_failed"];

pub const TOAST_DETAIL_MAX: usize = 240;
pub const BADGE_DETAIL_MAX: usize = 48;

/// Only a failed start or a crash toasts; other ends are silent.
#[must_use]
pub fn should_toast(reason: &str) -> bool {
    reason == "crashed" || FAILED_START_REASONS.contains(&reason)
}

fn ellipsize(text: &str, max: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max {
        return text.to_string();
    }
    let head: String = chars[..max - 1].iter().collect();
    format!("{head}…")
}

/// The toast's detail line, or `None` when the event carried nothing and the
/// caller should substitute its own "unknown error" wording.
#[must_use]
pub fn toast_detail(detail: Option<&str>) -> Option<String> {
    let raw = detail.map(str::trim).filter(|d| !d.is_empty())?;
    Some(ellipsize(raw, TOAST_DETAIL_MAX))
}

/// End-badge text: the label alone, or `label: <first line of detail>` for a
/// failed start.
#[must_use]
pub fn end_badge_text(reason: &str, label: &str, detail: Option<&str>) -> String {
    let Some(detail) = detail.filter(|d| !d.is_empty()) else {
        return label.to_string();
    };
    if !FAILED_START_REASONS.contains(&reason) {
        return label.to_string();
    }
    let line = detail.split('\n').next().unwrap_or("").trim();
    format!("{label}: {}", ellipsize(line, BADGE_DETAIL_MAX))
}

#[must_use]
pub fn session_href(session_id: &str) -> String {
    format!("/sessions/{}", encode_uri_component(session_id))
}
