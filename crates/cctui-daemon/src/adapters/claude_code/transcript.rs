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
/// `tail_once` then treats the missing file as silent success (`NotFound` →
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

/// Workflow-tool context for a subagent transcript discovered under
/// `subagents/workflows/<runId>/` (CCT-225). The Task tool's flat
/// `subagents/agent-*.jsonl` layout has no workflow, so this is `None` there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowContext {
    /// The workflow run id (e.g. `wf_fab6efd5-4bf`), = the `<runId>` dir name.
    pub run_id: String,
    /// Human workflow name (e.g. `deep-research`) from `workflows/<runId>.json`,
    /// if resolvable.
    pub name: Option<String>,
    /// `agentType` from the agent's `.meta.json` (e.g. `workflow-subagent`).
    pub agent_type: Option<String>,
}

/// A discovered subagent transcript: its agent id, transcript path, and (for
/// Workflow-tool agents) the enclosing workflow run context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentEntry {
    pub agent_id: String,
    pub path: PathBuf,
    pub workflow: Option<WorkflowContext>,
}

/// Discover every subagent transcript reachable from a parent session's
/// `subagents/` dir. Covers two layouts:
///
/// 1. Task tool — flat `subagents/agent-<agentId>.jsonl` (CCT-141).
/// 2. Workflow tool — nested `subagents/workflows/<runId>/agent-<agentId>.jsonl`
///    (CCT-225), with per-agent `.meta.json` (`agentType`) and a run-state
///    `workflows/<runId>.json` one level up under the session dir carrying the
///    workflow name. Nested `workflow()` calls reuse the same dir shape, so the
///    single-level glob below covers them too.
///
/// A missing directory (the common case — most sessions spawn no subagents)
/// yields an empty vec. The `agentId` is the stable subagent identifier Claude
/// assigns (also the id the Agent tool returns).
#[must_use]
pub fn discover_subagents(dir: &Path) -> Vec<SubagentEntry> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if let Some(agent_id) = name.strip_prefix("agent-").and_then(strip_jsonl) {
            out.push(SubagentEntry { agent_id: agent_id.to_owned(), path, workflow: None });
        } else if name == "workflows" && path.is_dir() {
            discover_workflow_subagents(&path, &mut out);
        }
    }
    out
}

fn strip_jsonl(name: &str) -> Option<&str> {
    name.strip_suffix(".jsonl")
}

/// Scan `subagents/workflows/` — one `<runId>/` dir per workflow run, each
/// holding `agent-<id>.jsonl` transcripts (+ `.meta.json` sidecars).
fn discover_workflow_subagents(workflows_dir: &Path, out: &mut Vec<SubagentEntry>) {
    let Ok(runs) = std::fs::read_dir(workflows_dir) else {
        return;
    };
    for run in runs.flatten() {
        let run_dir = run.path();
        if !run_dir.is_dir() {
            continue;
        }
        let Some(run_id) = run_dir.file_name().and_then(|n| n.to_str()).map(str::to_owned) else {
            continue;
        };
        let name = workflow_name(workflows_dir, &run_id);
        let Ok(agents) = std::fs::read_dir(&run_dir) else {
            continue;
        };
        for agent in agents.flatten() {
            let path = agent.path();
            let Some(fname) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some(agent_id) = fname.strip_prefix("agent-").and_then(strip_jsonl) else {
                continue;
            };
            let agent_type = agent_type_from_meta(&run_dir, agent_id);
            out.push(SubagentEntry {
                agent_id: agent_id.to_owned(),
                path: path.clone(),
                workflow: Some(WorkflowContext {
                    run_id: run_id.clone(),
                    name: name.clone(),
                    agent_type,
                }),
            });
        }
    }
}

/// Read the workflow name from the run-state file
/// `<session-dir>/workflows/<runId>.json` (one level up from the subagents
/// `workflows/` dir). The run state lives under `<session>/workflows/`, while
/// transcripts live under `<session>/subagents/workflows/` — siblings of the
/// session dir.
fn workflow_name(subagents_workflows_dir: &Path, run_id: &str) -> Option<String> {
    // subagents_workflows_dir = <session>/subagents/workflows
    // run state              = <session>/workflows/<runId>.json
    let session_dir = subagents_workflows_dir.parent()?.parent()?;
    let run_state = session_dir.join("workflows").join(format!("{run_id}.json"));
    let bytes = std::fs::read(run_state).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    value
        .get("name")
        .and_then(Value::as_str)
        .or_else(|| value.pointer("/script/name").and_then(Value::as_str))
        .or_else(|| value.pointer("/script/meta/name").and_then(Value::as_str))
        .map(str::to_owned)
}

/// Read `agentType` from a workflow agent's `.meta.json` sidecar
/// (`<runId>/agent-<id>.meta.json`).
fn agent_type_from_meta(run_dir: &Path, agent_id: &str) -> Option<String> {
    let meta_path = run_dir.join(format!("agent-{agent_id}.meta.json"));
    let bytes = std::fs::read(meta_path).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    value.get("agentType").and_then(Value::as_str).map(str::to_owned)
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

/// How far behind the persisted offset a reconciliation re-tail backs up
/// before re-reading (CCT-253). Large enough to recover several missed
/// turns' worth of transcript, small enough that the re-emitted volume
/// stays cheap (the server's content-hash dedup drops every dup).
pub const RECONCILE_BACKUP_BYTES: u64 = 64 * 1024;

/// Re-tail `path` from a checkpoint a fixed window BEHIND `persisted_offset`
/// to self-heal any gap left when an event was emitted but never persisted
/// server-side (a send dropped while the offset advanced) or when roster
/// churn re-homed the tail (CCT-253). Returns the parsed events; the caller
/// MUST NOT advance/persist any offset from this — it deliberately re-reads
/// already-seen lines, relying on the server's `ON CONFLICT … DO NOTHING`
/// dedup to drop the dups and surface only real gaps.
///
/// The checkpoint is realigned to a JSONL line boundary so parsing never
/// starts mid-line: `persisted_offset` always sits just after a `\n` (only
/// complete lines advance it in `tail_once`), but `persisted_offset -
/// backup` lands in the middle of an earlier line. We scan forward from the
/// backed-up position to the next `\n` and resume after it, so the first
/// line read is always whole. When the backup reaches the start of the file
/// (offset 0) we read from 0 directly — byte 0 is already a line boundary.
pub fn reconcile_tail(
    path: &Path,
    local_id: &str,
    persisted_offset: u64,
) -> std::io::Result<Vec<AdapterEvent>> {
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e),
    };
    let len = file.metadata()?.len();
    if len == 0 {
        return Ok(vec![]);
    }
    // Clamp the offset to the current length (the file may have rotated /
    // been truncated since the offset was taken) so the backup math stays
    // in bounds, then back up the window.
    let anchor = persisted_offset.min(len);
    let mut start = anchor.saturating_sub(RECONCILE_BACKUP_BYTES);

    file.seek(SeekFrom::Start(start))?;
    let mut reader = BufReader::new(file);

    // Realign to a line boundary: when we backed up into the middle of a
    // line (start > 0), discard the partial line by reading up to and
    // including the next '\n'. At start == 0 the position is already a
    // boundary, so skip the realignment.
    if start > 0 {
        let mut partial = String::new();
        let n = reader.read_line(&mut partial)?;
        start += n as u64;
        // If that "line" had no terminating '\n' it ran to EOF with no
        // complete line after the checkpoint — nothing to reconcile.
        if !partial.ends_with('\n') {
            return Ok(vec![]);
        }
    }

    let mut events = Vec::new();
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            break;
        }
        // Only parse complete lines; a truncated trailing line is left for
        // the regular tail to pick up once it's whole.
        if !line.ends_with('\n') {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        parse_line(local_id, &value, &mut events);
    }
    Ok(events)
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
    let message_id = message.and_then(|m| m.get("id")).and_then(Value::as_str);
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
    // Model id of what actually ran (`message.model`, e.g. "claude-opus-4-8").
    // Surfaces the model for sessions started without an explicit `--model`
    // flag; the server writes it only when `sessions.model` is still unset, so
    // an explicit `--model` alias keeps priority. Subagent transcripts carry
    // the parent's model too, so each gets labelled.
    if let Some(model) = message
        .and_then(|m| m.get("model"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|m| !m.is_empty())
    {
        out.push(AdapterEvent::SessionModel {
            local_id: local_id.to_owned(),
            model: model.to_owned(),
        });
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
                        "message_id": message_id,
                    }),
                });
            }
            Some("thinking") => {
                out.push(AdapterEvent::Message {
                    local_id: local_id.to_owned(),
                    payload: json!({
                        "role": "assistant_thinking",
                        "text": block.get("thinking").or_else(|| block.get("text")),
                        "message_id": message_id,
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
        found.sort_by(|a, b| a.agent_id.cmp(&b.agent_id));
        assert_eq!(
            found,
            vec![
                SubagentEntry {
                    agent_id: "a8412884de5cc5396".to_owned(),
                    path: dir.join("agent-a8412884de5cc5396.jsonl"),
                    workflow: None,
                },
                SubagentEntry {
                    agent_id: "b0c27d990208c793".to_owned(),
                    path: dir.join("agent-b0c27d990208c793.jsonl"),
                    workflow: None,
                },
            ]
        );
    }

    #[test]
    fn discover_subagents_finds_nested_workflow_agents() {
        // CCT-225: Workflow-tool agents live under subagents/workflows/<runId>/.
        let tmp = tempfile::tempdir().unwrap();
        // Lay out a realistic <session>/ tree.
        let session = tmp.path().join("-home-user").join("bea6c407");
        let subagents = session.join("subagents");
        let run_dir = subagents.join("workflows").join("wf_fab6efd5-4bf");
        std::fs::create_dir_all(&run_dir).unwrap();
        // Two workflow agents, one with a meta sidecar.
        std::fs::write(run_dir.join("agent-aaa.jsonl"), b"{}\n").unwrap();
        std::fs::write(run_dir.join("agent-bbb.jsonl"), b"{}\n").unwrap();
        std::fs::write(
            run_dir.join("agent-aaa.meta.json"),
            br#"{"agentType":"workflow-subagent"}"#,
        )
        .unwrap();
        // Run-state file carries the workflow name.
        let wf_state = session.join("workflows");
        std::fs::create_dir_all(&wf_state).unwrap();
        std::fs::write(wf_state.join("wf_fab6efd5-4bf.json"), br#"{"name":"deep-research"}"#)
            .unwrap();
        // Also a flat Task-tool agent alongside.
        std::fs::write(subagents.join("agent-flat.jsonl"), b"{}\n").unwrap();

        let mut found = discover_subagents(&subagents);
        found.sort_by(|a, b| a.agent_id.cmp(&b.agent_id));
        assert_eq!(found.len(), 3);

        let aaa = found.iter().find(|e| e.agent_id == "aaa").unwrap();
        assert_eq!(
            aaa.workflow,
            Some(WorkflowContext {
                run_id: "wf_fab6efd5-4bf".to_owned(),
                name: Some("deep-research".to_owned()),
                agent_type: Some("workflow-subagent".to_owned()),
            })
        );
        let bbb = found.iter().find(|e| e.agent_id == "bbb").unwrap();
        assert_eq!(
            bbb.workflow,
            Some(WorkflowContext {
                run_id: "wf_fab6efd5-4bf".to_owned(),
                name: Some("deep-research".to_owned()),
                agent_type: None, // no meta sidecar
            })
        );
        let flat = found.iter().find(|e| e.agent_id == "flat").unwrap();
        assert_eq!(flat.workflow, None);
    }

    #[test]
    fn workflow_name_falls_back_to_script_name() {
        let tmp = tempfile::tempdir().unwrap();
        let session = tmp.path().join("sess");
        let run_dir = session.join("subagents").join("workflows").join("wf_x");
        std::fs::create_dir_all(&run_dir).unwrap();
        std::fs::write(run_dir.join("agent-z.jsonl"), b"{}\n").unwrap();
        let wf_state = session.join("workflows");
        std::fs::create_dir_all(&wf_state).unwrap();
        // No top-level `name`, but script.name present.
        std::fs::write(wf_state.join("wf_x.json"), br#"{"script":{"name":"scripted"}}"#).unwrap();

        let found = discover_subagents(&session.join("subagents"));
        let z = found.iter().find(|e| e.agent_id == "z").unwrap();
        assert_eq!(z.workflow.as_ref().unwrap().name.as_deref(), Some("scripted"));
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
    fn reconcile_tail_closes_a_gap_behind_the_offset() {
        // Simulate the CCT-253 failure: the forward tail advanced (and
        // persisted) its offset past lines whose events never reached the
        // server. The reconcile re-tail backs up behind that offset and
        // re-emits them so the gap self-heals.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("t.jsonl");
        let line = |t: &str| {
            format!(
                r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":"{t}"}}]}}}}"#
            )
        };
        write_lines(&path, &[&line("a"), &line("b"), &line("c")]);
        // The persisted offset sits at EOF (all three lines "tailed"), but
        // say b and c never made it to the server.
        let persisted = std::fs::metadata(&path).unwrap().len();

        // A reconcile from a checkpoint behind the offset re-reads the lines.
        // The backup window is larger than the file, so it re-reads from the
        // start (byte 0) and re-emits every line.
        let events = reconcile_tail(&path, "s", persisted).unwrap();
        assert_eq!(events.len(), 3, "all lines behind the offset are re-emitted for dedup");
    }

    #[test]
    fn reconcile_tail_realigns_to_a_line_boundary() {
        // Build a transcript larger than the backup window so the checkpoint
        // (offset - RECONCILE_BACKUP_BYTES) genuinely lands MID-LINE, not at
        // byte 0. The realignment must discard that partial line and emit only
        // whole, fully-parsed messages — never a mangled half-line.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("t.jsonl");
        let line = |t: &str| {
            format!(
                r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":"{t}"}}]}}}}"#
            )
        };
        // ~80 bytes/line; ~1200 lines comfortably exceeds the 64 KiB window.
        let lines: Vec<String> = (0..1200).map(|i| line(&format!("msg-{i}"))).collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        write_lines(&path, &refs);
        let len = std::fs::metadata(&path).unwrap().len();
        assert!(len > RECONCILE_BACKUP_BYTES, "fixture must exceed the backup window");

        // Persisted offset at EOF -> checkpoint = len - 64KiB, mid-line.
        let events = reconcile_tail(&path, "s", len).unwrap();

        // Boundary safety: every event is a fully-parsed assistant message
        // (a mid-line start would have produced a fragment that fails JSON
        // parse and is dropped — so a corrupted bubble would never appear, but
        // the FIRST whole line after the checkpoint must parse cleanly).
        assert!(!events.is_empty(), "the window's worth of lines is re-emitted");
        for e in &events {
            assert!(matches!(e, AdapterEvent::Message { .. }));
        }
        // It re-emits only the window behind the offset, not the whole file.
        assert!(events.len() < lines.len(), "only the backup window is re-read, not all of it");
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
    fn emits_session_model_from_assistant_message() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("t.jsonl");
        write_lines(
            &path,
            &[
                r#"{"type":"assistant","message":{"id":"m1","model":"claude-opus-4-8","content":[{"type":"text","text":"hi"}]}}"#,
            ],
        );
        let (events, _) = tail_once(&path, "s", 0).unwrap();
        let model = events.iter().find_map(|e| match e {
            AdapterEvent::SessionModel { local_id, model } => {
                Some((local_id.clone(), model.clone()))
            }
            _ => None,
        });
        assert_eq!(model, Some(("s".to_owned(), "claude-opus-4-8".to_owned())));
    }

    #[test]
    fn no_session_model_when_field_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("t.jsonl");
        write_lines(
            &path,
            // older transcripts / sessions with no model field on the message
            &[
                r#"{"type":"assistant","message":{"id":"m1","content":[{"type":"text","text":"hi"}]}}"#,
            ],
        );
        let (events, _) = tail_once(&path, "s", 0).unwrap();
        assert!(!events.iter().any(|e| matches!(e, AdapterEvent::SessionModel { .. })));
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
