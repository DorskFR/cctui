//! `cctui-daemon whip-stop-hook` — the Claude Code `Stop` hook wired up by the
//! whip-mode (🐎) settings file.
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
///
/// `phrases_path` is the per-session whip phrase override file the
/// daemon writes at spawn. Absent / unreadable / malformed → the compiled
/// [`STALL_PHRASES`] defaults, so the zero-config path is unchanged.
#[must_use]
pub fn run(phrases_path: Option<&std::path::Path>) -> i32 {
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

    let config = load_phrase_config(phrases_path);
    let phrases = effective_phrases(config.as_ref());
    if let Some(matched) = first_stall_phrase(&phrases, &last) {
        match guidance_of(config.as_ref()) {
            Some(g) => {
                eprintln!("Whip mode (🐎): early-stop language detected (\"{matched}\").\n\n{g}");
            }
            None => eprintln!(
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
            ),
        }
        return 2;
    }
    0
}

/// Load the whip phrase override block from `path`, or `None` when the
/// path is absent / unreadable / not valid JSON — the caller then uses defaults.
fn load_phrase_config(path: Option<&std::path::Path>) -> Option<Value> {
    let bytes = std::fs::read(path?).ok()?;
    serde_json::from_slice::<Value>(&bytes).ok()
}

/// The effective, ordered, lowercase phrase list: `extend` (default)
/// appends the config's phrases to the compiled [`STALL_PHRASES`]; `replace`
/// swaps them out. `None` config → the compiled defaults.
fn effective_phrases(config: Option<&Value>) -> Vec<String> {
    let builtin = || STALL_PHRASES.iter().map(|p| (*p).to_owned());
    let Some(cfg) = config else { return builtin().collect() };
    let replace = cfg.get("mode").and_then(Value::as_str) == Some("replace");
    let mut out: Vec<String> = if replace { Vec::new() } else { builtin().collect() };
    if let Some(arr) = cfg.get("phrases").and_then(Value::as_array) {
        for p in arr.iter().filter_map(Value::as_str) {
            let p = p.trim().to_lowercase();
            if !p.is_empty() && !out.iter().any(|e| e == &p) {
                out.push(p);
            }
        }
    }
    out
}

/// The user's custom stderr guidance from the config, if any.
fn guidance_of(config: Option<&Value>) -> Option<String> {
    config
        .and_then(|c| c.get("guidance"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|g| !g.is_empty())
        .map(str::to_owned)
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

/// First phrase in `phrases` found in `text` (case-insensitive substring), or
/// `None`. `phrases` are already lowercase; `text` is lowercased here.
fn first_stall_phrase(phrases: &[String], text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    phrases.iter().find(|p| lower.contains(p.as_str())).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> Vec<String> {
        effective_phrases(None)
    }

    #[test]
    fn detects_handback_language() {
        let p = defaults();
        assert_eq!(
            first_stall_phrase(&p, "All set — let me know if you want more.").as_deref(),
            Some("let me know if")
        );
        assert_eq!(
            first_stall_phrase(&p, "This is a good stopping point.").as_deref(),
            Some("good stopping point")
        );
        assert_eq!(
            first_stall_phrase(&p, "Want me to take it to prod?").as_deref(),
            Some("want me to")
        );
        assert_eq!(
            first_stall_phrase(&p, "Ready for your review.").as_deref(),
            Some("ready for your review")
        );
    }

    #[test]
    fn allows_genuine_completion() {
        let p = defaults();
        assert_eq!(
            first_stall_phrase(&p, "Done. Tests pass and the deploy is verified in prod."),
            None
        );
        assert_eq!(first_stall_phrase(&p, "result: shipped v1.2.3, prod returns 200."), None);
    }

    #[test]
    fn case_insensitive() {
        assert_eq!(
            first_stall_phrase(&defaults(), "SHALL I continue?").as_deref(),
            Some("shall i")
        );
    }

    #[test]
    fn extend_appends_custom_phrases_keeping_defaults() {
        let cfg = serde_json::json!({
            "mode": "extend",
            "phrases": ["pour une autre session"]
        });
        let p = effective_phrases(Some(&cfg));
        assert_eq!(
            first_stall_phrase(&p, "Je garde ça pour une autre session.").as_deref(),
            Some("pour une autre session")
        );
        assert_eq!(first_stall_phrase(&p, "Shall I continue?").as_deref(), Some("shall i"));
    }

    #[test]
    fn replace_swaps_the_list_entirely() {
        let cfg = serde_json::json!({
            "mode": "replace",
            "phrases": ["prêt pour ta relecture"]
        });
        let p = effective_phrases(Some(&cfg));
        assert_eq!(
            first_stall_phrase(&p, "C'est prêt pour ta relecture.").as_deref(),
            Some("prêt pour ta relecture")
        );
        assert_eq!(first_stall_phrase(&p, "Ready for your review.").as_deref(), None);
    }

    #[test]
    fn missing_or_malformed_config_falls_back_to_defaults() {
        assert_eq!(effective_phrases(None), defaults());
        assert_eq!(load_phrase_config(Some(std::path::Path::new("/no/such/file"))), None);
        assert_eq!(effective_phrases(load_phrase_config(None).as_ref()), defaults());
    }

    #[test]
    fn custom_guidance_is_read_when_present() {
        let cfg =
            serde_json::json!({ "mode": "extend", "phrases": [], "guidance": "  keep going  " });
        assert_eq!(guidance_of(Some(&cfg)).as_deref(), Some("keep going"));
        assert_eq!(guidance_of(None), None);
        assert_eq!(guidance_of(Some(&serde_json::json!({ "mode": "extend" }))), None);
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
