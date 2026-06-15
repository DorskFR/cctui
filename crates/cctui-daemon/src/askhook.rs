//! `cctui-daemon ask-hook` — the Claude Code hook command wired up by the
//! managed settings file the daemon injects into every fleet-spawned session
//! (CCT-167).
//!
//! `AskUserQuestion` is the one interactive prompt the agents control socket
//! cannot surface live: the socket only reports coarse status (`state`/
//! `detail`), reporting `state:"done"` while a question is pending, and the
//! transcript flushes the `tool_use` block only *after* the turn advances. A
//! `PreToolUse` hook on `AskUserQuestion`, by contrast, fires the instant
//! before the form renders, with the full question payload on stdin.
//!
//! This subcommand reads that hook payload, formats the question, and forwards
//! it to the long-lived daemon over its local Unix socket. It is observe-only:
//! it prints nothing on stdout and always exits 0, so it never blocks or
//! alters the form Claude Code shows the user.

use std::fmt::Write as _;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};

/// Run the hook for `event` (`"pre"` = question appeared, `"post"` = answered).
///
/// Always returns `Ok(())` — a delivery failure must not break the user's
/// prompt — but logs to stderr so failures are still diagnosable.
///
/// `deny` (whip mode, CCT-352): after forwarding the question to the daemon
/// for visibility, emit a `PreToolUse` `deny` decision on stdout so the form
/// never renders and the model is told to decide and keep working instead of
/// asking. Only meaningful for the `pre` event.
pub fn run(event: &str, sock: &Path, deny: bool) -> anyhow::Result<()> {
    let mut input = String::new();
    if let Err(err) = std::io::stdin().read_to_string(&mut input) {
        eprintln!("cctui-daemon ask-hook: failed to read stdin: {err}");
        return Ok(());
    }
    let payload: Value = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(err) => {
            eprintln!("cctui-daemon ask-hook: stdin was not JSON: {err}");
            return Ok(());
        }
    };

    let Some(session_id) = payload.get("session_id").and_then(Value::as_str) else {
        eprintln!("cctui-daemon ask-hook: payload missing session_id");
        return Ok(());
    };

    let line = if event == "post" {
        json!({ "kind": "resolved", "session_id": session_id })
    } else {
        let tool_input = payload.get("tool_input");
        let question = format_questions(tool_input);
        // Forward the raw `questions` array too (header/options/multiSelect) so
        // the webui can render the interactive form live, not just the
        // flattened `question` text (CCT-181). Falls back to text alone if the
        // payload shape is unexpected.
        let questions = tool_input.and_then(|t| t.get("questions")).cloned();
        // The assistant prose preceding the question lives in the transcript as
        // its own earlier `type:"assistant"` text line (Claude writes one line
        // per content block); `transcript_path` is handed to us on stdin. Read
        // it so the live question card shows its context instead of being
        // answered blind (CCT-213). Without it the preamble would arrive up to
        // ~2s later over the daemon's transcript poll as a detached bubble.
        let preamble =
            payload.get("transcript_path").and_then(Value::as_str).and_then(read_preamble);
        json!({
            "kind": "ask",
            "session_id": session_id,
            "question": question,
            "questions": questions,
            "preamble": preamble,
        })
    };

    if let Err(err) = send(sock, &line.to_string()) {
        // The daemon may simply be down; that's not fatal for the user.
        eprintln!("cctui-daemon ask-hook: could not reach daemon at {}: {err}", sock.display());
    }

    // Whip mode: deny the tool so the form never renders. We forward the
    // question above purely for UI visibility, then return a `deny` decision
    // that Claude Code surfaces back to the model as the tool result.
    if deny && event != "post" {
        let decision = json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason":
                    "Whip mode (🐎): AskUserQuestion is disabled. Do not ask the user to \
                     choose, confirm, or review — decide using your best judgment and keep \
                     working. Only stop when the task is genuinely complete or you hit a \
                     concrete blocker you cannot resolve yourself.",
            },
        });
        println!("{decision}");
    }
    Ok(())
}

/// Read the assistant prose preceding the pending question from the transcript
/// at `path` (CCT-213).
///
/// Claude Code's transcript writer drains to disk on a 100ms timer
/// (`FLUSH_INTERVAL_MS`) independent of the turn loop, and writes one line per
/// content block — so the preamble `text` line lands ~100ms after it streams,
/// ~1s before the `tool_use` line that triggers this `PreToolUse` hook. We pause
/// briefly first to clear that sub-flush window (the hook can fire just before
/// the text line is drained), then scan the file. `None` on any read error or
/// when the model called the tool with no preceding text.
fn read_preamble(path: &str) -> Option<String> {
    // Let the preamble text line drain (FLUSH_INTERVAL_MS = 100). Cheap
    // insurance against the rare case where the hook beats the flush; the
    // delay is invisible next to the question card itself.
    std::thread::sleep(Duration::from_millis(120));
    let body = std::fs::read_to_string(path).ok()?;
    scan_preamble(&body)
}

/// Scan transcript `body` (newline-delimited JSON) backward for the assistant
/// prose of the current turn: the most recent `type:"assistant"` line carrying
/// a `{type:"text"}` block. Thinking-only and `tool_use`-only assistant lines
/// (the `AskUserQuestion` call itself) are skipped; a `type:"user"` line is a
/// turn boundary, so we stop there rather than grab text from an earlier turn.
fn scan_preamble(body: &str) -> Option<String> {
    for line in body.lines().rev() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match v.get("type").and_then(Value::as_str) {
            // A user / tool_result line is the turn boundary — if we reach it
            // before any assistant text, the tool was called with no preamble.
            Some("user") => return None,
            Some("assistant") => {
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
                    return Some(text);
                }
                // thinking-only / tool_use-only line: same turn, keep scanning.
            }
            _ => {} // summaries, attachments, etc. — skip, not a boundary.
        }
    }
    None
}

/// Render `tool_input.questions` into a readable plain-text prompt: each
/// question's header + text, then its options as bullet lines. This is the
/// show-only carrier for the existing `AskQuestion` event; a structured,
/// answer-from-webui form is a follow-up.
fn format_questions(tool_input: Option<&Value>) -> String {
    let Some(questions) = tool_input.and_then(|t| t.get("questions")).and_then(Value::as_array)
    else {
        return String::new();
    };
    let mut out = String::new();
    for q in questions {
        let header = q.get("header").and_then(Value::as_str).unwrap_or_default();
        let text = q.get("question").and_then(Value::as_str).unwrap_or_default();
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        if header.is_empty() {
            out.push_str(text);
        } else {
            let _ = write!(out, "{header}: {text}");
        }
        if let Some(options) = q.get("options").and_then(Value::as_array) {
            for opt in options {
                let label = opt.get("label").and_then(Value::as_str).unwrap_or_default();
                if label.is_empty() {
                    continue;
                }
                let desc = opt.get("description").and_then(Value::as_str).unwrap_or_default();
                if desc.is_empty() {
                    let _ = write!(out, "\n  • {label}");
                } else {
                    let _ = write!(out, "\n  • {label} — {desc}");
                }
            }
        }
    }
    out
}

/// Connect to the daemon socket and write one newline-delimited JSON line.
/// A connect timeout keeps the hook from ever hanging the agent's turn.
fn send(sock: &Path, line: &str) -> std::io::Result<()> {
    let mut stream = UnixStream::connect(sock)?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    stream.write_all(line.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_question_with_options() {
        let ti = json!({
            "questions": [{
                "header": "Scope",
                "question": "How should we handle it?",
                "options": [
                    {"label": "Show-only", "description": "minimal"},
                    {"label": "Full form"}
                ]
            }]
        });
        let s = format_questions(Some(&ti));
        assert!(s.contains("Scope: How should we handle it?"));
        assert!(s.contains("• Show-only — minimal"));
        assert!(s.contains("• Full form"));
    }

    #[test]
    fn formats_multiple_questions() {
        let ti = json!({
            "questions": [
                {"question": "First?"},
                {"question": "Second?"}
            ]
        });
        let s = format_questions(Some(&ti));
        assert!(s.contains("First?"));
        assert!(s.contains("Second?"));
        assert!(s.contains("\n\n"));
    }

    #[test]
    fn missing_questions_yields_empty() {
        assert_eq!(format_questions(None), "");
        assert_eq!(format_questions(Some(&json!({}))), "");
    }

    #[test]
    fn scan_preamble_finds_text_before_tool_use() {
        // CCT-213: the AskUserQuestion tool_use lands on its own line, preceded
        // by the assistant's text line in the same turn. Scanning backward must
        // skip the tool_use line and return the preceding prose.
        let body = concat!(
            r#"{"type":"user","message":{"content":"go"}}"#,
            "\n",
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Here is my analysis."}]}}"#,
            "\n",
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"AskUserQuestion","input":{}}]}}"#,
            "\n",
        );
        assert_eq!(scan_preamble(body).as_deref(), Some("Here is my analysis."));
    }

    #[test]
    fn scan_preamble_skips_thinking_only_lines() {
        let body = concat!(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"the recommendation"}]}}"#,
            "\n",
            r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"hmm"}]}}"#,
            "\n",
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"AskUserQuestion"}]}}"#,
            "\n",
        );
        assert_eq!(scan_preamble(body).as_deref(), Some("the recommendation"));
    }

    #[test]
    fn scan_preamble_stops_at_turn_boundary() {
        // The tool was called right after a tool_result/user line with no
        // preamble — must not reach back into the previous turn's text.
        let body = concat!(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"old turn text"}]}}"#,
            "\n",
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}"#,
            "\n",
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"AskUserQuestion"}]}}"#,
            "\n",
        );
        assert_eq!(scan_preamble(body), None);
    }

    #[test]
    fn scan_preamble_empty_or_garbage_is_none() {
        assert_eq!(scan_preamble(""), None);
        assert_eq!(scan_preamble("not json\n{also not\n"), None);
    }
}
