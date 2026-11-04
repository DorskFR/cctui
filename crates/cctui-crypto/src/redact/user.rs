//! `<redacted>…</redacted>` spans, masked regardless of the detector set and its allowlist.

use std::borrow::Cow;

use serde_json::Value;

pub const USER_PLACEHOLDER: &str = "[REDACTED:user]";
pub const USER_CATEGORY: &str = "user";
/// A shorter value still masks its own span but is never echo-matched, so
/// `<redacted>a</redacted>` cannot wipe every "a".
pub const MIN_ECHO_LITERAL_LEN: usize = 4;

const OPEN: &str = "<redacted>";
const CLOSE: &str = "</redacted>";

/// Leaves that carry an event's shape rather than its content. Echo literals
/// never rewrite them, so a value like `user` cannot break deserialization.
const STRUCTURAL_KEYS: &[&str] = &[
    "adapter_id",
    "command_id",
    "id",
    "item_id",
    "kind",
    "line_id",
    "local_id",
    "message_id",
    "request_id",
    "role",
    "session_id",
    "tool_use_id",
    "turn_id",
    "type",
    "uuid",
];

struct Span {
    start: usize,
    inner: (usize, usize),
    end: usize,
}

/// Closed, non-empty spans in order. Tags match case-insensitively; the first
/// close ends a span; an unclosed open masks nothing.
fn spans(s: &str) -> Vec<Span> {
    let mut out = Vec::new();
    if !s.contains("</") {
        return out;
    }
    let lower = s.to_ascii_lowercase();
    let mut from = 0;
    while let Some(open) = lower[from..].find(OPEN) {
        let start = from + open;
        let inner_start = start + OPEN.len();
        let Some(close) = lower[inner_start..].find(CLOSE) else { break };
        let inner_end = inner_start + close;
        let end = inner_end + CLOSE.len();
        if inner_end > inner_start {
            out.push(Span { start, inner: (inner_start, inner_end), end });
        }
        from = end;
    }
    out
}

/// The text inside each closed, non-empty span.
#[must_use]
pub fn user_span_values(s: &str) -> Vec<&str> {
    spans(s).into_iter().map(|sp| &s[sp.inner.0..sp.inner.1]).collect()
}

#[must_use]
pub fn has_user_spans(s: &str) -> bool {
    !spans(s).is_empty()
}

fn mask_spans(s: &str) -> (Cow<'_, str>, usize) {
    let found = spans(s);
    if found.is_empty() {
        return (Cow::Borrowed(s), 0);
    }
    let mut out = String::with_capacity(s.len());
    let mut last = 0;
    for sp in &found {
        out.push_str(&s[last..sp.start]);
        out.push_str(USER_PLACEHOLDER);
        last = sp.end;
    }
    out.push_str(&s[last..]);
    (Cow::Owned(out), found.len())
}

/// `s` with every tagged span, tags included, replaced by [`USER_PLACEHOLDER`].
#[must_use]
pub fn mask_user_spans(s: &str) -> Cow<'_, str> {
    mask_spans(s).0
}

/// The per-session set of span values to mask wherever they are echoed back.
/// Kept longest first so a literal never pre-empts a longer one containing it.
#[derive(Debug, Default, Clone)]
pub struct UserLiterals {
    values: Vec<String>,
}

impl UserLiterals {
    #[must_use]
    pub const fn new() -> Self {
        Self { values: Vec::new() }
    }

    /// Adds `value` (trimmed) unless it is too short or would match inside the
    /// placeholder itself. Returns whether it was added.
    pub fn insert(&mut self, value: &str) -> bool {
        let value = value.trim();
        if value.chars().count() < MIN_ECHO_LITERAL_LEN
            || USER_PLACEHOLDER.contains(value)
            || self.values.iter().any(|v| v == value)
        {
            return false;
        }
        let at = self.values.partition_point(|v| v.len() >= value.len());
        self.values.insert(at, value.to_owned());
        true
    }

    /// Adds every span value of `text`.
    pub fn extend_from_text(&mut self, text: &str) {
        for value in user_span_values(text) {
            self.insert(value);
        }
    }

    pub fn merge(&mut self, other: Self) {
        for value in other.values {
            self.insert(&value);
        }
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    #[must_use]
    pub fn contains(&self, value: &str) -> bool {
        self.values.iter().any(|v| v == value)
    }

    fn mask<'a>(&self, s: Cow<'a, str>) -> (Cow<'a, str>, usize) {
        let mut out = s;
        let mut count = 0;
        for lit in &self.values {
            let n = out.matches(lit.as_str()).count();
            if n > 0 {
                out = Cow::Owned(out.replace(lit.as_str(), USER_PLACEHOLDER));
                count += n;
            }
        }
        (out, count)
    }
}

/// Mask tagged spans, then every echo of `literals`. Returns the text and the
/// number of substitutions.
#[must_use]
pub fn mask_user_text<'a>(s: &'a str, literals: &UserLiterals) -> (Cow<'a, str>, usize) {
    let (out, spans) = mask_spans(s);
    let (out, echoes) = literals.mask(out);
    (out, spans + echoes)
}

/// [`mask_user_text`] over every string leaf of `value`. Structural leaves
/// (`type`, `role`, ids…) only get span masking.
pub fn mask_user_json(value: &mut Value, literals: &UserLiterals) -> usize {
    walk(value, literals, false)
}

fn walk(value: &mut Value, literals: &UserLiterals, structural: bool) -> usize {
    match value {
        Value::String(s) => {
            let none = UserLiterals::new();
            let lits = if structural { &none } else { literals };
            let (out, n) = mask_user_text(s, lits);
            if n > 0 {
                *s = out.into_owned();
            }
            n
        }
        Value::Array(items) => items.iter_mut().map(|v| walk(v, literals, false)).sum(),
        Value::Object(map) => map
            .iter_mut()
            .map(|(k, v)| walk(v, literals, STRUCTURAL_KEYS.contains(&k.as_str())))
            .sum(),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::redact::{compile, redact_json};

    fn masked(s: &str) -> String {
        mask_user_spans(s).into_owned()
    }

    #[test]
    fn a_single_span_is_masked_with_its_tags() {
        assert_eq!(masked("pw is <redacted>hunter2-xyz</redacted> ok"), "pw is [REDACTED:user] ok");
    }

    #[test]
    fn several_spans_are_each_masked() {
        assert_eq!(
            masked("<redacted>a</redacted> and <redacted>bcde</redacted>!"),
            "[REDACTED:user] and [REDACTED:user]!"
        );
    }

    #[test]
    fn a_multiline_span_is_masked() {
        assert_eq!(masked("x <redacted>line1\nline2\r\n</redacted> y"), "x [REDACTED:user] y");
    }

    #[test]
    fn tags_match_case_insensitively() {
        assert_eq!(
            masked("<ReDaCtEd>v</REDACTED> <REDACTED>w</redacted>"),
            "[REDACTED:user] [REDACTED:user]"
        );
    }

    #[test]
    fn the_first_close_ends_a_span() {
        assert_eq!(
            masked("<redacted>a<redacted>b</redacted>c</redacted>"),
            "[REDACTED:user]c</redacted>"
        );
    }

    #[test]
    fn an_unclosed_span_is_left_as_written() {
        let s = "keep <redacted>this open";
        assert_eq!(masked(s), s);
        assert!(matches!(mask_user_spans(s), Cow::Borrowed(_)));
        assert_eq!(masked("<redacted>x</redacted> <redacted>y"), "[REDACTED:user] <redacted>y");
    }

    #[test]
    fn an_empty_span_does_nothing() {
        let s = "a <redacted></redacted> b";
        assert_eq!(masked(s), s);
        assert!(user_span_values(s).is_empty());
    }

    #[test]
    fn masking_is_idempotent() {
        let once = masked("a <redacted>secret-value</redacted> b");
        assert_eq!(masked(&once), once);
        let mut lits = UserLiterals::new();
        lits.insert("secret-value");
        let (twice, n) = mask_user_text(&once, &lits);
        assert_eq!(twice, once);
        assert_eq!(n, 0);
        let mut v = json!(once.clone());
        assert_eq!(redact_json(&mut v, &compile(true, &[], b"k")), 0);
        assert_eq!(v, json!(once));
    }

    #[test]
    fn a_span_is_masked_even_with_scrubbing_disabled() {
        let patterns = compile(false, &[], b"k");
        assert!(patterns.is_empty());
        let mut v = json!({"text": "<redacted>hunter2-xyz</redacted>"});
        redact_json(&mut v, &patterns);
        assert_eq!(mask_user_json(&mut v, &UserLiterals::new()), 1);
        assert_eq!(v, json!({"text": USER_PLACEHOLDER}));
    }

    #[test]
    fn a_span_is_masked_even_when_its_value_is_allowlisted() {
        let token = "ghp_ABCDEFGHIJKLMNOPQRSTUVWX0123";
        let patterns = compile(true, &[], b"k").with_allowlist([token.to_owned()]);
        let mut v = json!(format!("use <redacted>{token}</redacted>"));
        mask_user_json(&mut v, &UserLiterals::new());
        redact_json(&mut v, &patterns);
        assert_eq!(v, json!("use [REDACTED:user]"));
    }

    #[test]
    fn echo_literals_skip_short_and_placeholder_values() {
        let mut lits = UserLiterals::new();
        lits.extend_from_text(
            "<redacted>a</redacted> <redacted>abc</redacted> <redacted>user</redacted>",
        );
        assert!(lits.is_empty());
        lits.extend_from_text("<redacted>  hunter2-xyz \n</redacted>");
        assert!(lits.contains("hunter2-xyz"));
        assert_eq!(masked("<redacted>a</redacted>"), USER_PLACEHOLDER);
    }

    #[test]
    fn echoes_are_masked_longest_literal_first() {
        let mut lits = UserLiterals::new();
        lits.insert("hunter2");
        lits.insert("hunter2-xyz");
        let (out, n) = mask_user_text("say hunter2-xyz then hunter2", &lits);
        assert_eq!(out, "say [REDACTED:user] then [REDACTED:user]");
        assert_eq!(n, 2);
    }

    #[test]
    fn literals_never_rewrite_structural_leaves() {
        let mut lits = UserLiterals::new();
        lits.insert("assistant");
        let mut v = json!({"role": "assistant", "text": "I am the assistant"});
        assert_eq!(mask_user_json(&mut v, &lits), 1);
        assert_eq!(v, json!({"role": "assistant", "text": "I am the [REDACTED:user]"}));
    }

    #[test]
    fn multibyte_text_around_a_span_survives() {
        assert_eq!(masked("é<REDACTED>ü</redacted>漢"), "é[REDACTED:user]漢");
    }
}
