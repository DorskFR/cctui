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
pub fn run(event: &str, sock: &Path) -> anyhow::Result<()> {
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
        let question = format_questions(payload.get("tool_input"));
        json!({ "kind": "ask", "session_id": session_id, "question": question })
    };

    if let Err(err) = send(sock, &line.to_string()) {
        // The daemon may simply be down; that's not fatal for the user.
        eprintln!("cctui-daemon ask-hook: could not reach daemon at {}: {err}", sock.display());
    }
    Ok(())
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
}
