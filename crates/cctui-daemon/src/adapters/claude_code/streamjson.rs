//! Shared stream-json plumbing for the headless claude-code drivers: frame
//! parsing ([`parse_stream_line`]), stdin user envelopes
//! ([`user_message_envelope`]) and a stderr ring for crash detail
//! ([`spawn_stderr_ring`]).
//!
//! `stream_event` deltas are ignored: the coalesced `assistant` frame carries
//! the final text, and forwarding deltas would double-emit. An error frame
//! ends the run with [`EndReason::Crashed`].

use std::collections::VecDeque;
use std::sync::Arc;

use cctui_proto::adapter::{AdapterEvent, EndReason};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::ChildStderr;
use tokio::sync::Mutex;

use super::transcript;

/// What a single stream-json frame told us, beyond any [`AdapterEvent`]s it
/// produced: whether it ends the run.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct StreamOutcome {
    /// `Some` when this frame terminates the run: a `result` frame
    /// ([`EndReason::Completed`]) or an error frame ([`EndReason::Crashed`]).
    pub end: Option<EndReason>,
}

/// Parse one stream-json stdout line into [`AdapterEvent`]s (appended to
/// `out`) plus a [`StreamOutcome`]. Non-JSON / unknown frames are ignored
/// (empty outcome, no events) so a stray log line can't abort the run.
pub(super) fn parse_stream_line(
    local_id: &str,
    line: &str,
    out: &mut Vec<AdapterEvent>,
) -> StreamOutcome {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return StreamOutcome::default();
    }
    let Ok(v) = serde_json::from_str::<Value>(trimmed) else {
        tracing::debug!(?trimmed, "ignoring non-JSON stream-json line");
        return StreamOutcome::default();
    };
    let kind = v.get("type").and_then(Value::as_str).unwrap_or_default();
    match kind {
        "system" => parse_system(local_id, &v, out),
        // `assistant`/`user` frames carry the same `message.content` shape as
        // transcript lines, so reuse the canonical normalization.
        "assistant" | "user" => {
            transcript::parse_line(local_id, &v, out);
            StreamOutcome::default()
        }
        // Incremental SSE deltas: the coalesced `assistant` frame is the
        // source of truth, so drop partials to avoid double-emitting.
        "stream_event" => StreamOutcome::default(),
        "result" => StreamOutcome { end: Some(result_end_reason(&v)) },
        // A top-level error frame (some CLI builds emit `type:"error"`).
        "error" => StreamOutcome { end: Some(error_end(&v)) },
        other => {
            tracing::debug!(kind = other, "ignoring unknown stream-json frame");
            StreamOutcome::default()
        }
    }
}

/// A `system` frame is either the `init` handshake (carries the session id +
/// model) or a `subtype:"error"` failure.
fn parse_system(local_id: &str, v: &Value, out: &mut Vec<AdapterEvent>) -> StreamOutcome {
    match v.get("subtype").and_then(Value::as_str) {
        Some("init") => {
            // The model the run resolved to (e.g. "claude-opus-4-8"); fills
            // `sessions.model` for runs launched without an explicit --model,
            // mirroring the transcript SessionModel path.
            if let Some(model) =
                v.get("model").and_then(Value::as_str).map(str::trim).filter(|m| !m.is_empty())
            {
                out.push(AdapterEvent::SessionModel {
                    local_id: local_id.to_owned(),
                    model: model.to_owned(),
                });
            }
            StreamOutcome::default()
        }
        Some("error") => StreamOutcome { end: Some(error_end(v)) },
        other => {
            transcript::record_unknown("unknown-streamjson-system", other.unwrap_or("<none>"));
            StreamOutcome::default()
        }
    }
}

/// How many trailing child stderr lines to retain for crash diagnostics.
/// When `claude` dies unexpectedly these lines are the only clue why, so they
/// ride along in the [`EndReason::Crashed`] detail.
pub const STDERR_RING: usize = 40;

pub type StderrRing = Arc<Mutex<VecDeque<String>>>;

/// Drain a child's stderr into a bounded ring (and the log at debug level).
pub fn spawn_stderr_ring(stderr: ChildStderr, driver: &'static str) -> StderrRing {
    let ring: StderrRing = Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_RING)));
    let writer = ring.clone();
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if line.trim().is_empty() {
                continue;
            }
            tracing::debug!(target: "claude_stderr", driver, "{line}");
            let mut guard = writer.lock().await;
            if guard.len() == STDERR_RING {
                guard.pop_front();
            }
            guard.push_back(line);
        }
    });
    ring
}

/// `"; last stderr:\n…"` suffix from the ring, or empty when nothing was logged.
pub async fn stderr_tail(ring: &StderrRing) -> String {
    let lines: Vec<String> = { ring.lock().await.iter().cloned().collect() };
    if lines.is_empty() { String::new() } else { format!("; last stderr:\n{}", lines.join("\n")) }
}

/// Crash detail for an abnormal child exit: `<what> exited (<status>)` — the
/// status carries the exit code or the terminating signal — plus the stderr tail.
pub fn exit_detail(what: &str, status: std::process::ExitStatus, tail: &str) -> String {
    format!("{what} exited ({status}){tail}")
}

/// [`EndReason::Crashed`] carrying an error frame's `message`/`error` text
/// (best-effort) so the failure isn't anonymous.
fn error_end(v: &Value) -> EndReason {
    let detail = v
        .get("message")
        .or_else(|| v.get("error"))
        .and_then(Value::as_str)
        .unwrap_or("stream-json error frame")
        .to_owned();
    EndReason::Crashed { detail }
}

/// Map a `result` frame's `subtype` to an [`EndReason`]. `success` →
/// [`EndReason::Completed`]; any error subtype (`error_max_turns`,
/// `error_during_execution`, …) → [`EndReason::Crashed`].
fn result_end_reason(v: &Value) -> EndReason {
    let ok = v.get("subtype").and_then(Value::as_str) == Some("success")
        && !v.get("is_error").and_then(Value::as_bool).unwrap_or(false);
    if ok {
        EndReason::Completed
    } else {
        let subtype =
            v.get("subtype").and_then(Value::as_str).unwrap_or("stream-json result error");
        let detail = match v.get("result").and_then(Value::as_str).map(str::trim) {
            Some(result) if !result.is_empty() => format!("{subtype}: {result}"),
            _ => subtype.to_owned(),
        };
        EndReason::Crashed { detail }
    }
}

/// Build the `--input-format stream-json` user-message line the CLI reads on
/// stdin: `{"type":"user","message":{"role":"user","content":<content>}}`.
///
/// `content` is passed through as-is so callers can send either a plain text
/// turn (`json!(text)`) or a structured content-block array (tool results,
/// attachments). The returned `Value` is one line; the driver appends `\n`.
pub(super) fn user_message_envelope(content: &Value) -> Value {
    json!({
        "type": "user",
        "message": { "role": "user", "content": content },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn exit_detail_carries_status_and_stderr_tail() {
        let mut child = tokio::process::Command::new("sh")
            .arg("-c")
            .arg("for i in $(seq 1 45); do echo \"line $i\" >&2; done; exit 3")
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let ring = spawn_stderr_ring(child.stderr.take().unwrap(), "test_stderr");
        let status = child.wait().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let detail = exit_detail("claude -p", status, &stderr_tail(&ring).await);
        assert!(
            detail.starts_with("claude -p exited (exit status: 3); last stderr:\n"),
            "{detail}"
        );
        assert!(!detail.contains("line 5\n"), "ring must drop lines beyond the last 40: {detail}");
        assert!(detail.contains("line 6\n"), "{detail}");
        assert!(detail.ends_with("line 45"), "{detail}");
        assert_eq!(exit_detail("claude -p", status, ""), "claude -p exited (exit status: 3)");
    }

    #[test]
    fn unknown_system_subtypes_are_counted_by_namespaced_label() {
        let mut out = Vec::new();
        for line in [r#"{"type":"system","subtype":"tachyon_burst"}"#, r#"{"type":"system"}"#] {
            let outcome = parse_stream_line("s", line, &mut out);
            assert_eq!(outcome, StreamOutcome::default());
        }
        assert!(out.is_empty(), "unknown system subtypes must not fabricate events");
        for label in ["unknown-streamjson-system:tachyon_burst", "unknown-streamjson-system:<none>"]
        {
            let count = transcript::transcript_drop_tally()
                .into_iter()
                .find(|(k, _)| k == label)
                .map_or(0, |(_, v)| v);
            assert!(count >= 1, "{label} must be counted");
        }
    }

    #[test]
    fn result_error_includes_result_text() {
        let v = json!({ "type": "result", "subtype": "error_during_execution", "is_error": true, "result": "Invalid model" });
        match result_end_reason(&v) {
            EndReason::Crashed { detail } => {
                assert_eq!(detail, "error_during_execution: Invalid model");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    // Captured `claude --output-format stream-json --verbose` stdout lines.
    const INIT_LINE: &str = r#"{"type":"system","subtype":"init","session_id":"11111111-2222-3333-4444-555555555555","model":"claude-opus-4-8","cwd":"/tmp/x","tools":["Bash","Read"],"permissionMode":"default"}"#;
    const ASSISTANT_LINE: &str = r#"{"type":"assistant","message":{"id":"msg_01","model":"claude-opus-4-8","role":"assistant","content":[{"type":"text","text":"Hello there"},{"type":"tool_use","id":"tu_1","name":"Bash","input":{"command":"ls"}}],"usage":{"input_tokens":120,"output_tokens":45,"cache_read_input_tokens":900,"cache_creation_input_tokens":10}}}"#;
    const USER_TOOL_RESULT_LINE: &str = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu_1","content":"file.txt","is_error":false}]}}"#;
    const STREAM_EVENT_LINE: &str = r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Hel"}}}"#;
    const RESULT_SUCCESS_LINE: &str = r#"{"type":"result","subtype":"success","is_error":false,"session_id":"11111111-2222-3333-4444-555555555555","num_turns":2,"result":"done"}"#;
    const RESULT_ERROR_LINE: &str = r#"{"type":"result","subtype":"error_max_turns","is_error":true,"session_id":"11111111-2222-3333-4444-555555555555"}"#;
    const SYSTEM_ERROR_LINE: &str = r#"{"type":"system","subtype":"error","message":"boom"}"#;

    fn parse(line: &str) -> (Vec<AdapterEvent>, StreamOutcome) {
        let mut out = Vec::new();
        let outcome = parse_stream_line("L1", line, &mut out);
        (out, outcome)
    }

    #[test]
    fn init_carries_model() {
        let (events, outcome) = parse(INIT_LINE);
        assert!(outcome.end.is_none());
        match events.as_slice() {
            [AdapterEvent::SessionModel { local_id, model }] => {
                assert_eq!(local_id, "L1");
                assert_eq!(model, "claude-opus-4-8");
            }
            other => panic!("expected one SessionModel, got {other:?}"),
        }
    }

    #[test]
    fn assistant_frame_maps_text_tooluse_tokens_and_model() {
        let (events, outcome) = parse(ASSISTANT_LINE);
        assert!(outcome.end.is_none());
        assert!(
            events.iter().any(|e| matches!(
                e,
                AdapterEvent::TokenUsage {
                    input_tokens: 120,
                    output_tokens: 45,
                    cache_read_tokens: 900,
                    cache_creation_tokens: 10,
                    ..
                }
            )),
            "token usage mapped: {events:?}"
        );
        assert!(events.iter().any(|e| matches!(e, AdapterEvent::SessionModel { .. })));
        assert!(events.iter().any(|e| matches!(e, AdapterEvent::Message { .. })));
        assert!(events.iter().any(|e| matches!(e, AdapterEvent::ToolUse { .. })));
    }

    #[test]
    fn user_frame_maps_tool_result() {
        let (events, outcome) = parse(USER_TOOL_RESULT_LINE);
        assert!(outcome.end.is_none());
        match events.as_slice() {
            [AdapterEvent::ToolUse { payload, .. }] => {
                assert_eq!(payload["kind"], "tool_result");
                assert_eq!(payload["tool_use_id"], "tu_1");
            }
            other => panic!("expected one tool_result ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn stream_event_deltas_are_dropped() {
        let (events, outcome) = parse(STREAM_EVENT_LINE);
        assert!(events.is_empty());
        assert_eq!(outcome, StreamOutcome::default());
    }

    #[test]
    fn result_success_ends_completed() {
        let (events, outcome) = parse(RESULT_SUCCESS_LINE);
        assert!(events.is_empty());
        assert_eq!(outcome.end, Some(EndReason::Completed));
    }

    #[test]
    fn result_error_subtype_ends_failed() {
        let (_events, outcome) = parse(RESULT_ERROR_LINE);
        match outcome.end {
            Some(EndReason::Crashed { detail }) => assert_eq!(detail, "error_max_turns"),
            other => panic!("expected Crashed, got {other:?}"),
        }
    }

    #[test]
    fn system_error_ends_failed() {
        let (_events, outcome) = parse(SYSTEM_ERROR_LINE);
        match outcome.end {
            Some(EndReason::Crashed { detail }) => assert_eq!(detail, "boom"),
            other => panic!("expected Crashed, got {other:?}"),
        }
    }

    #[test]
    fn garbage_and_blank_lines_are_ignored() {
        assert_eq!(parse("not json").1, StreamOutcome::default());
        assert!(parse("not json").0.is_empty());
        assert_eq!(parse("   ").1, StreamOutcome::default());
        // Unknown but valid frame.
        assert_eq!(parse(r#"{"type":"frobnicate"}"#).1, StreamOutcome::default());
    }

    #[test]
    fn user_message_envelope_wraps_text() {
        let env = user_message_envelope(&json!("hi there"));
        assert_eq!(env["type"], "user");
        assert_eq!(env["message"]["role"], "user");
        assert_eq!(env["message"]["content"], "hi there");
    }

    #[test]
    fn user_message_envelope_passes_through_blocks() {
        let blocks = json!([{"type":"text","text":"a"}]);
        let env = user_message_envelope(&blocks);
        assert_eq!(env["message"]["content"], blocks);
    }
}
