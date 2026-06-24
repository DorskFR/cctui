//! `cctui-daemon whip-stop-hook` — the Claude Code `Stop` hook wired up by the
//! whip-mode (🐎) settings file (CCT-352).
//!
//! Whip mode exists to keep a fleet worker running until the work is genuinely
//! done. The model's strongest stalling lever is to *stop* — to end a turn with
//! "let me know if you'd like…", "ready for your review", "good stopping point",
//! and hand back instead of finishing. This `Stop` hook reads the final
//! assistant message and, if it reads as a graceful early exit, exits non-zero
//! with guidance on stderr — which Claude Code feeds back as a reason to keep
//! going rather than stop.
//!
//! Exit codes follow the Claude Code hook contract: `0` lets the stop through,
//! `2` blocks it and surfaces stderr to the model. `stop_hook_active` guards
//! against an infinite loop — once we've already blocked this stop once, we let
//! the next one through.

use std::io::Read;

use serde_json::Value;

/// Phrases (matched case-insensitively as substrings of the final assistant
/// message) that read as a graceful early exit / hand-back. Tuned from the
/// machine-local `block-early-stop.sh` plus whip-specific stalling tells.
const STALL_PHRASES: &[&str] = &[
    // Scope-punting.
    "out of scope",
    "not in scope",
    "beyond the scope",
    "beyond scope",
    "left this for",
    "left that for",
    "leaving this for",
    "leaving that for",
    "for a future",
    "for a follow-up",
    "for a followup",
    "for a follow up",
    "next session",
    "future session",
    "can be done later",
    "can be addressed later",
    "can be fixed later",
    "punting on",
    "pre-existing issue",
    "pre-existing bug",
    "pre-existing failure",
    // Stopping / pausing.
    "stopping here",
    "pausing here",
    "pausing for now",
    "good stopping point",
    "natural stopping point",
    "good place to stop",
    "good place to pause",
    "good session",
    "good checkpoint",
    "checkpoint:",
    // Hand-back / deference.
    "handing this back",
    "handing it back",
    "handing back",
    "over to you",
    "your call",
    "let me know if",
    "let me know how",
    "let me know whether",
    "feel free to",
    "ready for your review",
    "ready for review",
    "for you to review",
    "for your review",
    "waiting for your",
    "wait for your",
    "would you like me to",
    "do you want me to",
    "want me to",
    "shall i",
    "should i proceed",
    "if you'd like",
    "if you would like",
    "happy to continue",
    "happy to keep going",
];

/// Run the `Stop` hook. Reads the hook JSON on stdin; returns the process exit
/// code (`0` = allow stop, `2` = block stop and emit guidance on stderr).
#[must_use]
pub fn run() -> i32 {
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        return 0;
    }
    let payload: Value = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(_) => return 0,
    };

    // Already blocked this stop once — let it through to avoid a loop.
    if payload.get("stop_hook_active").and_then(Value::as_bool).unwrap_or(false) {
        return 0;
    }

    let Some(path) = payload.get("transcript_path").and_then(Value::as_str) else {
        return 0;
    };
    let Ok(body) = std::fs::read_to_string(path) else {
        return 0;
    };
    let Some(last) = last_assistant_text(&body) else {
        return 0;
    };

    if let Some(matched) = first_stall_phrase(&last) {
        eprintln!(
            "Whip mode (🐎): early-stop language detected (\"{matched}\").\n\
             \n\
             Do not stop yet. Either:\n\
               1. Actually finish the work — run the failing thing, fix the \
             \"pre-existing\" issue, verify end to end — or\n\
               2. If you genuinely cannot proceed, state the one concrete \
             blocker in a single line and stop. Do not narrate a graceful exit, \
             ask the user to review, or hand work back.\n\
             \n\
             Re-examine the task. What concretely remains? Keep going."
        );
        return 2;
    }
    0
}

/// The most recent `type:"assistant"` message's concatenated text blocks, or
/// `None` if the transcript has no assistant text.
fn last_assistant_text(body: &str) -> Option<String> {
    let mut last: Option<String> = None;
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let text = v
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(Value::as_array)
            .map(|blocks| {
                blocks
                    .iter()
                    .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                    .filter_map(|b| b.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        if !text.trim().is_empty() {
            last = Some(text);
        }
    }
    last
}

/// First stall phrase found in `text` (case-insensitive substring), or `None`.
fn first_stall_phrase(text: &str) -> Option<&'static str> {
    let lower = text.to_lowercase();
    STALL_PHRASES.iter().copied().find(|p| lower.contains(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_handback_language() {
        assert_eq!(
            first_stall_phrase("All set — let me know if you want more."),
            Some("let me know if")
        );
        assert_eq!(
            first_stall_phrase("This is a good stopping point."),
            Some("good stopping point")
        );
        assert_eq!(first_stall_phrase("Want me to take it to prod?"), Some("want me to"));
        assert_eq!(first_stall_phrase("Ready for your review."), Some("ready for your review"));
    }

    #[test]
    fn allows_genuine_completion() {
        assert_eq!(
            first_stall_phrase("Done. Tests pass and the deploy is verified in prod."),
            None
        );
        assert_eq!(first_stall_phrase("result: shipped v1.2.3, prod returns 200."), None);
    }

    #[test]
    fn case_insensitive() {
        assert_eq!(first_stall_phrase("SHALL I continue?"), Some("shall i"));
    }

    #[test]
    fn last_assistant_text_picks_final_turn() {
        let body = concat!(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"first"}]}}"#,
            "\n",
            r#"{"type":"user","message":{"content":"go"}}"#,
            "\n",
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"final answer"}]}}"#,
            "\n",
        );
        assert_eq!(last_assistant_text(body).as_deref(), Some("final answer"));
    }

    #[test]
    fn last_assistant_text_none_when_empty() {
        assert_eq!(last_assistant_text(""), None);
        assert_eq!(last_assistant_text("garbage\n{not json"), None);
    }
}
