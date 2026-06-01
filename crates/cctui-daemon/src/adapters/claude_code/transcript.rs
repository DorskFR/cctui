//! Tail the standard Claude transcript at
//! `~/.claude/projects/<encoded-cwd>/<sessionId>.jsonl`.
//!
//! The `subscribe` op (deferred) and the `timeline.jsonl` daemon log
//! both lack tool-call detail (§6.2 of the protocol doc). The transcript
//! here is the canonical source for `tool_use` blocks, agent text, and
//! `post_turn_summary` events.
//!
//! Design choice: rather than running a separate `notify` watcher per
//! session, we tail incrementally on the driver's 2s poll tick using a
//! persisted byte offset. The transcript file grows monotonically (only
//! `/clear` truncates), so seek-and-read-new is sufficient and avoids
//! the rename / debounce complications a watcher would carry.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use cctui_proto::adapter::AdapterEvent;
use serde_json::{Value, json};

/// Encode a working directory into the path-segment Claude uses under
/// `~/.claude/projects/`. Per protocol §6.1: replace every `/` AND every
/// `.` with `-`. Empirically `/.` collapses to `--`.
///
/// Claude normalizes the cwd before deriving this segment: a trailing slash
/// is dropped, so `/a/b/` and `/a/b` map to the SAME `<encoded>` dir. We must
/// match that. A cctui-dispatched session whose `working_dir` carried a
/// trailing slash (the UI sends `/home/you/proj/`) otherwise encodes to
/// `-home-you-proj-` (an extra trailing dash) — a directory that never exists.
/// `tail_once` then treats the missing file as silent success (NotFound →
/// empty), so the session shows live status but "No events yet" forever
/// (CCT-196).
#[must_use]
pub fn encode_cwd(cwd: &str) -> String {
    cwd.trim_end_matches('/')
        .chars()
        .map(|c| match c {
            '/' | '.' => '-',
            other => other,
        })
        .collect()
}

/// Resolve the transcript path for a session.
#[must_use]
pub fn transcript_path(projects_root: &Path, cwd: &str, session_id: &str) -> PathBuf {
    projects_root.join(encode_cwd(cwd)).join(format!("{session_id}.jsonl"))
}

/// Directory holding per-subagent (Task-tool) transcripts for a parent
/// session: `<encoded-cwd>/<parent-session-id>/subagents/`. Derived from
/// the parent's own transcript path `<encoded-cwd>/<parent-session-id>.jsonl`
/// by stripping the `.jsonl` extension and descending into `subagents/`
/// (CCT-141).
#[must_use]
pub fn subagents_dir(parent_transcript: &Path) -> PathBuf {
    parent_transcript.with_extension("").join("subagents")
}

/// List `(agent_id, path)` for every `agent-<agentId>.jsonl` transcript in
/// `dir`. A missing directory (the common case — most sessions spawn no
/// subagents) yields an empty vec. The `agentId` is the stable subagent
/// identifier Claude assigns (also the id the Agent tool returns).
#[must_use]
pub fn discover_subagents(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if let Some(agent_id) =
            name.strip_prefix("agent-").and_then(|rest| rest.strip_suffix(".jsonl"))
        {
            out.push((agent_id.to_owned(), path.clone()));
        }
    }
    out
}

/// Read new lines from `path` starting at `offset`. Returns the parsed
/// events plus the new offset. If the file is shorter than `offset`
/// (rotation / `/clear`), the offset is reset to 0 and the file is
/// re-read from the beginning.
pub fn tail_once(
    path: &Path,
    local_id: &str,
    mut offset: u64,
) -> std::io::Result<(Vec<AdapterEvent>, u64)> {
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok((vec![], offset));
        }
        Err(e) => return Err(e),
    };
    let len = file.metadata()?.len();
    if len < offset {
        // Rotation: start over.
        offset = 0;
    }
    if len == offset {
        return Ok((vec![], offset));
    }
    file.seek(SeekFrom::Start(offset))?;
    let mut reader = BufReader::new(file);
    let mut events = Vec::new();
    let mut new_offset = offset;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            break;
        }
        // Only advance the persisted offset over complete lines (ending
        // in '\n'); a truncated trailing line will be re-read next tick.
        if !line.ends_with('\n') {
            break;
        }
        new_offset += n as u64;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
            tracing::debug!(?trimmed, "ignoring non-JSON transcript line");
            continue;
        };
        parse_line(local_id, &value, &mut events);
    }
    Ok((events, new_offset))
}

fn parse_line(local_id: &str, line: &Value, out: &mut Vec<AdapterEvent>) {
    let kind = line.get("type").and_then(Value::as_str).unwrap_or_default();
    // CCT-159: `/compact` appends an `isCompactSummary` line (a `type:"user"`
    // entry whose content is the auto-generated summary) into the SAME
    // transcript — no session-id rotation, so the CCT-158 rotation marker never
    // fires. Surface it as a dedicated compact event instead of letting it
    // render as a giant user-typed bubble.
    if line.get("isCompactSummary").and_then(Value::as_bool).unwrap_or(false) {
        out.push(AdapterEvent::Message {
            local_id: local_id.to_owned(),
            payload: json!({ "role": "compact_summary", "text": compact_summary_text(line) }),
        });
        return;
    }
    match kind {
        "assistant" => parse_assistant(local_id, line, out),
        "user" => parse_user(local_id, line, out),
        "post_turn_summary" => {
            out.push(AdapterEvent::Message {
                local_id: local_id.to_owned(),
                payload: json!({
                    "role": "summary",
                    "status_category": line.get("status_category"),
                    "status_detail": line.get("status_detail"),
                    "needs_action": line.get("needs_action"),
                }),
            });
        }
        _ => {
            // attachment, permission-mode, worktree-state, pr-link,
            // ai-title, agent-name, agent-setting, last-prompt — silently
            // skipped for v1. Specific carriers may be added later as
            // their semantics become useful to the UI.
        }
    }
}

fn parse_assistant(local_id: &str, line: &Value, out: &mut Vec<AdapterEvent>) {
    let message = line.get("message");
    // Token usage — one row per assistant message, idempotent on
    // `(session_id, message_id)`. Skip when either the usage block or
    // the message id is missing (older transcripts).
    if let (Some(usage), Some(msg_id)) = (
        message.and_then(|m| m.get("usage")),
        message.and_then(|m| m.get("id")).and_then(Value::as_str),
    ) {
        let pick = |k: &str| usage.get(k).and_then(Value::as_u64).unwrap_or(0);
        let input = pick("input_tokens");
        let output = pick("output_tokens");
        let cache_read = pick("cache_read_input_tokens");
        let cache_creation = pick("cache_creation_input_tokens");
        if input | output | cache_read | cache_creation > 0 {
            out.push(AdapterEvent::TokenUsage {
                local_id: local_id.to_owned(),
                message_id: msg_id.to_owned(),
                input_tokens: input,
                output_tokens: output,
                cache_read_tokens: cache_read,
                cache_creation_tokens: cache_creation,
            });
        }
    }
    let Some(content) = message.and_then(|m| m.get("content")).and_then(Value::as_array) else {
        return;
    };
    for block in content {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                out.push(AdapterEvent::Message {
                    local_id: local_id.to_owned(),
                    payload: json!({
                        "role": "assistant",
                        "text": block.get("text"),
                    }),
                });
            }
            Some("thinking") => {
                out.push(AdapterEvent::Message {
                    local_id: local_id.to_owned(),
                    payload: json!({
                        "role": "assistant_thinking",
                        "text": block.get("thinking").or_else(|| block.get("text")),
                    }),
                });
            }
            Some("tool_use") => {
                out.push(AdapterEvent::ToolUse {
                    local_id: local_id.to_owned(),
                    payload: json!({
                        "id": block.get("id"),
                        "tool": block.get("name"),
                        "input": block.get("input"),
                    }),
                });
            }
            _ => {}
        }
    }
}

/// Harness tags that wrap content injected *to* the agent (not typed by the
/// human): background-task notifications, slash-command expansions, bash
/// passthrough, injected reminders. Claude's top-level `isMeta` flag covers
/// some of these (system-reminder, autonomous-loop wake-ups) but not all
/// (task-notification, `<command-name>` are `isMeta:false`), so we OR the two
/// signals together. These tokens are fixed strings Claude Code emits, so the
/// match is exact, not a fuzzy guess.
const META_TAGS: [&str; 8] = [
    "<task-notification",
    "<system-reminder",
    "<command-name",
    "<command-message",
    "<local-command",
    "<bash-input",
    "<bash-stdout",
    "<bash-stderr",
];

/// Whether a user-role transcript message is really a system/agent-directed
/// message rather than human input. `is_meta_line` is Claude's top-level
/// `isMeta`; `text` is the message body.
fn user_text_is_meta(is_meta_line: bool, text: &str) -> bool {
    is_meta_line || {
        let t = text.trim_start();
        META_TAGS.iter().any(|tag| t.starts_with(tag))
    }
}

/// Extract the summary text from a `/compact` line. The content lives under
/// `message.content`, which is either a plain string or an array of `text`
/// blocks (same shape as a normal user message) — join the latter.
fn compact_summary_text(line: &Value) -> String {
    let Some(content) = line.get("message").and_then(|m| m.get("content")) else {
        return String::new();
    };
    if let Some(s) = content.as_str() {
        return s.to_owned();
    }
    let Some(blocks) = content.as_array() else { return String::new() };
    blocks
        .iter()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|b| b.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_user(local_id: &str, line: &Value, out: &mut Vec<AdapterEvent>) {
    // User lines can be plain text or carry tool_result blocks.
    let Some(content) = line.get("message").and_then(|m| m.get("content")) else {
        return;
    };
    let is_meta_line = line.get("isMeta").and_then(Value::as_bool).unwrap_or(false);
    if let Some(text) = content.as_str() {
        out.push(AdapterEvent::Message {
            local_id: local_id.to_owned(),
            payload: json!({"role": "user", "text": text, "meta": user_text_is_meta(is_meta_line, text)}),
        });
        return;
    }
    let Some(blocks) = content.as_array() else { return };
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                let text = block.get("text").and_then(Value::as_str).unwrap_or_default();
                out.push(AdapterEvent::Message {
                    local_id: local_id.to_owned(),
                    payload: json!({"role": "user", "text": text, "meta": user_text_is_meta(is_meta_line, text)}),
                });
            }
            Some("tool_result") => {
                out.push(AdapterEvent::ToolUse {
                    local_id: local_id.to_owned(),
                    payload: json!({
                        "kind": "tool_result",
                        "tool_use_id": block.get("tool_use_id"),
                        "content": block.get("content"),
                        "is_error": block.get("is_error"),
                    }),
                });
            }
            _ => {}
        }
    }
}

/// Persistent (`session_id` → byte offset) store.
///
/// Persisted at `~/.config/cctui/transcript-offsets.json` so daemon
/// restarts don't replay already-forwarded events. Best-effort: a load
/// failure is treated as an empty map (events get replayed once, which
/// the server can dedupe by content hash if desired).
#[derive(Debug, Default)]
pub struct OffsetStore {
    path: Option<PathBuf>,
    map: HashMap<String, u64>,
}

impl OffsetStore {
    /// Open the store at the default path
    /// (`$XDG_CONFIG_HOME/cctui/transcript-offsets.json` or
    /// `~/.config/cctui/...`).
    #[must_use]
    #[allow(dead_code)]
    pub fn open_default() -> Self {
        let dir = dirs::config_dir().map(|d| d.join("cctui"));
        let path = dir.as_ref().map(|d| d.join("transcript-offsets.json"));
        Self::open(path)
    }

    #[must_use]
    pub fn open(path: Option<PathBuf>) -> Self {
        let map = path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self { path, map }
    }

    #[must_use]
    pub fn get(&self, key: &str) -> u64 {
        self.map.get(key).copied().unwrap_or(0)
    }

    pub fn set(&mut self, key: String, offset: u64) {
        self.map.insert(key, offset);
    }

    /// Persist to disk. Failures logged + swallowed — a missed write
    /// only costs a one-time replay on the next restart.
    pub fn flush(&self) {
        let Some(path) = &self.path else { return };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_vec_pretty(&self.map) {
            Ok(bytes) => {
                if let Err(err) = std::fs::write(path, bytes) {
                    tracing::warn!(%err, ?path, "failed to persist transcript offsets");
                }
            }
            Err(err) => tracing::warn!(%err, "failed to serialise transcript offsets"),
        }
    }
}

#[must_use]
pub fn default_projects_root() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")).join(".claude").join("projects")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn encode_cwd_replaces_slash_and_dot() {
        assert_eq!(encode_cwd("/Users/me/.claude/feedbacks"), "-Users-me--claude-feedbacks");
        assert_eq!(encode_cwd("/tmp/test"), "-tmp-test");
        assert_eq!(encode_cwd("/a.b/c"), "-a-b-c");
    }

    #[test]
    fn encode_cwd_drops_trailing_slash() {
        // CCT-196: a dispatched session's working_dir often carries a trailing
        // slash (`/home/you/proj/`). Claude normalizes it away before deriving
        // the projects-dir segment, so we must too — otherwise the encoded dir
        // gets a spurious trailing dash and the transcript is never found.
        assert_eq!(encode_cwd("/home/gtax/dev/gtax/"), "-home-gtax-dev-gtax");
        assert_eq!(encode_cwd("/home/gtax/dev/gtax"), "-home-gtax-dev-gtax");
        // Multiple trailing slashes collapse the same way.
        assert_eq!(encode_cwd("/tmp/test//"), "-tmp-test");
    }

    #[test]
    fn transcript_path_is_built_correctly() {
        let p = transcript_path(Path::new("/projects"), "/Users/me", "abc-123");
        assert_eq!(p, PathBuf::from("/projects/-Users-me/abc-123.jsonl"));
    }

    #[test]
    fn subagents_dir_descends_from_parent_transcript() {
        let parent = transcript_path(Path::new("/projects"), "/Users/me", "sess-1");
        assert_eq!(subagents_dir(&parent), PathBuf::from("/projects/-Users-me/sess-1/subagents"));
    }

    #[test]
    fn discover_subagents_lists_agent_files_only() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        std::fs::write(dir.join("agent-a8412884de5cc5396.jsonl"), b"{}\n").unwrap();
        std::fs::write(dir.join("agent-b0c27d990208c793.jsonl"), b"{}\n").unwrap();
        // Non-matching entries are ignored.
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        std::fs::write(dir.join("agent-partial.json"), b"x").unwrap();
        let mut found = discover_subagents(dir);
        found.sort();
        assert_eq!(
            found,
            vec![
                ("a8412884de5cc5396".to_owned(), dir.join("agent-a8412884de5cc5396.jsonl")),
                ("b0c27d990208c793".to_owned(), dir.join("agent-b0c27d990208c793.jsonl")),
            ]
        );
    }

    #[test]
    fn discover_subagents_missing_dir_is_empty() {
        assert!(discover_subagents(Path::new("/no/such/dir")).is_empty());
    }

    fn write_lines(path: &Path, lines: &[&str]) {
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
        for l in lines {
            f.write_all(l.as_bytes()).unwrap();
            f.write_all(b"\n").unwrap();
        }
    }

    #[test]
    fn tail_emits_assistant_text_and_tool_use() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("t.jsonl");
        write_lines(
            &path,
            &[
                r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hello"},{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"ls"}}]}}"#,
            ],
        );
        let (events, offset) = tail_once(&path, "sess1", 0).unwrap();
        assert_eq!(events.len(), 2);
        assert!(matches!(&events[0], AdapterEvent::Message { .. }));
        assert!(matches!(&events[1], AdapterEvent::ToolUse { .. }));
        assert!(offset > 0);
    }

    #[test]
    fn tail_is_incremental() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("t.jsonl");
        write_lines(
            &path,
            &[r#"{"type":"assistant","message":{"content":[{"type":"text","text":"a"}]}}"#],
        );
        let (e1, off1) = tail_once(&path, "s", 0).unwrap();
        assert_eq!(e1.len(), 1);
        write_lines(
            &path,
            &[r#"{"type":"assistant","message":{"content":[{"type":"text","text":"b"}]}}"#],
        );
        let (e2, off2) = tail_once(&path, "s", off1).unwrap();
        assert_eq!(e2.len(), 1);
        assert!(off2 > off1);
    }

    #[test]
    fn tail_handles_rotation() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("t.jsonl");
        // Write a bunch of lines so the offset is meaningfully large.
        for _ in 0..10 {
            write_lines(
                &path,
                &[r#"{"type":"assistant","message":{"content":[{"type":"text","text":"x"}]}}"#],
            );
        }
        let (_, off) = tail_once(&path, "s", 0).unwrap();
        // Simulate /clear: truncate and write a single short line.
        std::fs::write(&path, b"").unwrap();
        write_lines(
            &path,
            &[r#"{"type":"assistant","message":{"content":[{"type":"text","text":"y"}]}}"#],
        );
        let (events, new_off) = tail_once(&path, "s", off).unwrap();
        assert_eq!(events.len(), 1);
        assert!(new_off > 0 && new_off < off + 1024); // restarted from 0
    }

    #[test]
    fn tail_skips_trailing_partial_line() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("t.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(
            br#"{"type":"assistant","message":{"content":[{"type":"text","text":"complete"}]}}"#,
        )
        .unwrap();
        f.write_all(b"\n").unwrap();
        f.write_all(br#"{"type":"assistant","message":{"content":[{"type":"text","text":"partial"#)
            .unwrap();
        // no trailing newline
        let (events, off) = tail_once(&path, "s", 0).unwrap();
        assert_eq!(events.len(), 1, "only the complete line should be emitted");
        // Append the rest.
        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(b"\"}]}}\n").unwrap();
        let (events2, _) = tail_once(&path, "s", off).unwrap();
        assert_eq!(events2.len(), 1, "partial line replayed once complete");
    }

    #[test]
    fn user_tool_result_emits_tool_use_event() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("t.jsonl");
        write_lines(
            &path,
            &[
                r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"ok","is_error":false}]}}"#,
            ],
        );
        let (events, _) = tail_once(&path, "s", 0).unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], AdapterEvent::ToolUse { .. }));
    }

    #[test]
    fn user_meta_detection() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("t.jsonl");
        write_lines(
            &path,
            &[
                // genuine human input → not meta
                r#"{"type":"user","message":{"content":"do the thing"}}"#,
                // harness tag, isMeta absent → meta via tag match
                r#"{"type":"user","message":{"content":"<task-notification><status>completed</status></task-notification>"}}"#,
                // injected reminder marked by Claude's isMeta, no tag
                r##"{"type":"user","isMeta":true,"message":{"content":[{"type":"text","text":"# Autonomous loop check"}]}}"##,
            ],
        );
        let (events, _) = tail_once(&path, "s", 0).unwrap();
        let metas: Vec<bool> = events
            .iter()
            .filter_map(|e| match e {
                AdapterEvent::Message { payload, .. } => {
                    Some(payload.get("meta").and_then(Value::as_bool).unwrap_or(false))
                }
                _ => None,
            })
            .collect();
        assert_eq!(metas, vec![false, true, true]);
    }

    #[test]
    fn compact_summary_emits_compact_role_not_user() {
        // CCT-159: /compact appends an `isCompactSummary` user line in place
        // (no rotation). It must surface as a `compact_summary` message, not a
        // plain user bubble.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("t.jsonl");
        write_lines(
            &path,
            &[
                r#"{"type":"user","isCompactSummary":true,"message":{"role":"user","content":[{"type":"text","text":"summary of the conversation"}]}}"#,
            ],
        );
        let (events, _) = tail_once(&path, "s", 0).unwrap();
        assert_eq!(events.len(), 1);
        match &events[0] {
            AdapterEvent::Message { payload, .. } => {
                assert_eq!(payload.get("role").and_then(Value::as_str), Some("compact_summary"));
                assert_eq!(
                    payload.get("text").and_then(Value::as_str),
                    Some("summary of the conversation")
                );
            }
            other => panic!("expected compact_summary Message, got {other:?}"),
        }
    }

    #[test]
    fn compact_summary_string_content() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("t.jsonl");
        write_lines(
            &path,
            &[
                r#"{"type":"user","isCompactSummary":true,"message":{"role":"user","content":"plain string summary"}}"#,
            ],
        );
        let (events, _) = tail_once(&path, "s", 0).unwrap();
        assert_eq!(events.len(), 1);
        match &events[0] {
            AdapterEvent::Message { payload, .. } => {
                assert_eq!(
                    payload.get("text").and_then(Value::as_str),
                    Some("plain string summary")
                );
            }
            other => panic!("expected compact_summary Message, got {other:?}"),
        }
    }

    #[test]
    fn assistant_usage_emits_token_usage_event() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("t.jsonl");
        write_lines(
            &path,
            &[
                r#"{"type":"assistant","message":{"id":"msg_abc","content":[{"type":"text","text":"hi"}],"usage":{"input_tokens":100,"output_tokens":50,"cache_read_input_tokens":900,"cache_creation_input_tokens":10}}}"#,
            ],
        );
        let (events, _) = tail_once(&path, "s", 0).unwrap();
        let tu = events
            .iter()
            .find(|e| matches!(e, AdapterEvent::TokenUsage { .. }))
            .expect("expected TokenUsage event");
        match tu {
            AdapterEvent::TokenUsage {
                message_id,
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_creation_tokens,
                ..
            } => {
                assert_eq!(message_id, "msg_abc");
                assert_eq!(*input_tokens, 100);
                assert_eq!(*output_tokens, 50);
                assert_eq!(*cache_read_tokens, 900);
                assert_eq!(*cache_creation_tokens, 10);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn assistant_without_usage_emits_no_token_event() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("t.jsonl");
        write_lines(
            &path,
            &[
                r#"{"type":"assistant","message":{"id":"msg_x","content":[{"type":"text","text":"hi"}]}}"#,
            ],
        );
        let (events, _) = tail_once(&path, "s", 0).unwrap();
        assert!(!events.iter().any(|e| matches!(e, AdapterEvent::TokenUsage { .. })));
    }

    #[test]
    fn offset_store_persists() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("offsets.json");
        let mut s = OffsetStore::open(Some(path.clone()));
        s.set("sess1".into(), 42);
        s.flush();
        let s2 = OffsetStore::open(Some(path));
        assert_eq!(s2.get("sess1"), 42);
    }
}
