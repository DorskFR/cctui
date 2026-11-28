use std::collections::BTreeMap;

use super::{CompiledPatterns, redact_string};

pub const STREAM_HOLD_MAX: usize = 4096;

const KEY_BLOCK_OPEN: &str = "-----BEGIN ";
const KEY_BLOCK_CLOSE: &str = "-----END ";

/// `NAME =` and `label:` widen the hold to the line: the detector needs that
/// prefix in the same haystack as the value.
fn secret_hold_from(s: &str) -> usize {
    let floor = s.ceil_char_boundary(s.len().saturating_sub(STREAM_HOLD_MAX));
    if let Some(at) = s[floor..].rfind(KEY_BLOCK_OPEN) {
        let open = floor + at;
        if !s[open..].contains(KEY_BLOCK_CLOSE) {
            return open;
        }
    }
    let word = s
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace())
        .map_or(0, |(i, c)| i + c.len_utf8());
    let line = s[..word].rfind('\n').map_or(0, |i| i + 1);
    let before = s[line..word].trim_end();
    let cut = if before.ends_with('=') || before.ends_with(':') { line } else { word };
    cut.max(floor)
}

/// Returns `(redacted text to show now, raw tail)`. The tail is unredacted:
/// feed it back in front of the next chunk, never show it.
#[must_use]
pub fn split_streamed_secrets(s: &str, patterns: &CompiledPatterns) -> (String, String) {
    if patterns.is_empty() {
        return (s.to_owned(), String::new());
    }
    let cut = secret_hold_from(s);
    let (ready, hold) = s.split_at(cut);
    let emit =
        redact_string(ready, patterns, &mut BTreeMap::new()).unwrap_or_else(|| ready.to_owned());
    (emit, hold.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::redact::compile;

    const KEY: &[u8] = b"test-key-32-bytes-test-key-32byt";

    fn stream(chunks: &[&str]) -> (String, String) {
        let p = compile(true, &[], KEY);
        let mut shown = String::new();
        let mut carry = String::new();
        for chunk in chunks {
            let (emit, hold) = split_streamed_secrets(&(carry + chunk), &p);
            shown.push_str(&emit);
            carry = hold;
        }
        (shown, carry)
    }

    #[test]
    fn a_token_split_across_chunks_is_never_shown_raw() {
        let (shown, held) = stream(&["the token is ghp_AB", "CDEFGHIJKLMNOPQR", "STUVWX0123 ok"]);
        assert!(shown.contains("[REDACTED:github_token"), "{shown}");
        assert!(!shown.contains("ghp_AB"), "{shown}");
        assert!(!shown.contains("MNOPQR"), "{shown}");
        assert_eq!(held, "ok");
    }

    #[test]
    fn no_prefix_of_a_split_token_is_emitted() {
        let token = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123";
        for cut in 1..token.len() {
            let (head, tail) = token.split_at(cut);
            let (shown, _) = stream(&["key ", head, tail, " done"]);
            assert!(!shown.contains("sk-ant"), "cut {cut}: {shown}");
            assert!(shown.contains("[REDACTED:anthropic"), "cut {cut}: {shown}");
        }
    }

    #[test]
    fn plain_prose_flows_up_to_the_last_word() {
        let (shown, held) = stream(&["Hello the", "re, how are", " you"]);
        assert_eq!(shown, "Hello there, how are ");
        assert_eq!(held, "you");
    }

    #[test]
    fn an_assignment_line_waits_for_its_value() {
        let (shown, held) = stream(&["run:\nFIREWORKS_API_KEY = ", "zX9qLmNb2v8Kd4Rt", "\nnext"]);
        assert!(shown.contains("[REDACTED:env_assignment"), "{shown}");
        assert!(!shown.contains("zX9qLmNb"), "{shown}");
        assert_eq!(held, "next");
    }

    #[test]
    fn a_signed_url_keeps_its_query_anchor() {
        let (shown, _) =
            stream(&["see https://n8n.example/wait/1?sig", "nature=5ce8ef8a", "8245 now"]);
        assert!(!shown.contains("5ce8ef8a"), "{shown}");
        assert!(shown.contains("[REDACTED:url_signed_param"), "{shown}");
    }

    #[test]
    fn an_open_key_block_is_held_until_it_closes() {
        let (shown, held) = stream(&[
            "key:\n-----BEGIN RSA PRIVATE KEY-----\nMIIab",
            "c123\n",
            "-----END RSA PRIVATE KEY----- done",
        ]);
        assert!(shown.contains("[REDACTED:private_key]"), "{shown}");
        assert!(!shown.contains("MIIab"), "{shown}");
        assert_eq!(held, "done");
    }

    #[test]
    fn an_endless_run_is_capped() {
        let run = "a".repeat(STREAM_HOLD_MAX * 2);
        let (shown, held) = stream(&[&run]);
        assert!(held.len() <= STREAM_HOLD_MAX);
        assert_eq!(shown.len() + held.len(), run.len());
    }

    #[test]
    fn multibyte_text_is_cut_on_char_boundaries() {
        let text = format!("{}é", "é".repeat(STREAM_HOLD_MAX));
        let (shown, held) = stream(&[&text]);
        assert_eq!(format!("{shown}{held}"), text);
    }

    #[test]
    fn disabled_patterns_hold_nothing() {
        let (emit, hold) =
            split_streamed_secrets("ghp_AB", &crate::redact::CompiledPatterns::disabled());
        assert_eq!((emit.as_str(), hold.as_str()), ("ghp_AB", ""));
    }
}
