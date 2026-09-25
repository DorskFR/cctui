use std::path::{Path, PathBuf};

use cctui_proto::adapter::AdapterEvent;
use serde_json::{Value, json};

use super::RECONCILE_BACKUP_BYTES;

/// Recursively collect rollout files under `dir`. Codex nests sessions as
/// `sessions/YYYY/MM/DD/rollout-*.jsonl`; depth is capped so a symlink cycle or
/// unexpected tree can't spin the scan forever. Flat files directly under the
/// root (used by tests and older layouts) are still picked up at depth 0.
pub(super) fn collect_rollout_files(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 6 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rollout_files(&path, depth + 1, out);
        } else if path.is_file() {
            out.push(path);
        }
    }
}

/// Canonical thread id for a rollout file. The identity is the
/// `session_meta` payload `id` (a UUID), not the filename — the app-server
/// registry and `thread/list` inventory key off the same UUID, so deriving it
/// from the file avoids a second, filename-shaped local id for one thread.
/// Falls back to a UUID embedded in the filename, then the bare stem.
pub(super) fn derive_local_id(path: &Path) -> String {
    if let Some(id) = session_meta_id(path) {
        return id;
    }
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("unknown");
    uuid_from_stem(stem).unwrap_or_else(|| stem.to_owned())
}

/// Read the `session_meta` line (first line of a well-formed rollout) and return
/// its lowercased payload `id`. Scans a bounded prefix in case the meta isn't
/// strictly first; returns `None` for files that carry no `session_meta`.
fn session_meta_id(path: &Path) -> Option<String> {
    session_meta_payload(path)
        .as_ref()
        .and_then(|p| p.get("id").and_then(Value::as_str).map(str::to_ascii_lowercase))
}

/// The `session_meta` line's `payload` object, or `None` for a rollout that
/// carries no `session_meta`. Scans a bounded prefix in case the meta isn't
/// strictly first.
fn session_meta_payload(path: &Path) -> Option<Value> {
    use std::io::{BufRead, BufReader};
    let file = std::fs::File::open(path).ok()?;
    let reader = BufReader::new(file);
    for line in reader.lines().take(64).map_while(Result::ok) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(trimmed) else { continue };
        if value.get("type").and_then(Value::as_str) == Some("session_meta") {
            return value.get("payload").cloned();
        }
    }
    None
}

/// Stamped by a launcher through `CODEX_INTERNAL_ORIGINATOR_OVERRIDE`, which codex
/// copies verbatim into `session_meta.originator`.
const LAUNCHER_ORIGINATOR_PREFIX: &str = "cctui-parent.";

pub(super) struct RolloutLink {
    pub(super) source: Option<String>,
    pub(super) subagent_parent: Option<String>,
    pub(super) launcher_parent: Option<String>,
}

/// Reduce a rollout's `session_meta` to how it links to a parent. Mirrors the
/// `thread/list` inventory extraction so both discovery paths agree on which
/// rollouts are subagents and who their parent is. Codex calls a plain `codex
/// exec` a root thread, so the stamped originator is the only thing tying one
/// back to the cctui session that launched it.
pub(super) fn rollout_link(path: &Path) -> RolloutLink {
    let Some(payload) = session_meta_payload(path) else {
        return RolloutLink { source: None, subagent_parent: None, launcher_parent: None };
    };
    let source = payload.get("source").and_then(crate::adapters::codex::thread_list::parse_source);
    let subagent_parent = crate::adapters::codex::thread_list::parse_parent(&payload);
    let launcher_parent = payload
        .get("originator")
        .and_then(Value::as_str)
        .and_then(|s| s.strip_prefix(LAUNCHER_ORIGINATOR_PREFIX))
        .filter(|s| !s.is_empty())
        .map(crate::adapters::codex::thread_list::canonical_id);
    RolloutLink { source, subagent_parent, launcher_parent }
}

/// Extract a canonical 8-4-4-4-12 hex UUID from a rollout filename stem such as
/// `rollout-2026-07-12T01-25-55-019f51ff-f19f-7ed2-bf2a-bbb0d5cc5b90` by scanning
/// hyphen-separated segments for the five-group UUID window.
fn uuid_from_stem(stem: &str) -> Option<String> {
    let segs: Vec<&str> = stem.split('-').collect();
    let is_hex = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_hexdigit());
    for w in segs.windows(5) {
        if [8, 4, 4, 4, 12] == [w[0].len(), w[1].len(), w[2].len(), w[3].len(), w[4].len()]
            && w.iter().all(|s| is_hex(s))
        {
            return Some(w.join("-").to_ascii_lowercase());
        }
    }
    None
}

/// Parse from `offset` to the last complete line, returning the events and the
/// offset that line ends at. A truncated trailing line never advances the
/// offset, so the next scan re-reads it whole.
pub(super) fn read_new_lines(
    path: &Path,
    offset: u64,
    local_id: &str,
) -> std::io::Result<(Vec<AdapterEvent>, u64)> {
    use std::io::{BufRead, BufReader, Seek, SeekFrom};

    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    if len <= offset {
        return Ok((vec![], offset));
    }
    file.seek(SeekFrom::Start(offset))?;
    let mut reader = BufReader::new(file);
    let mut out = Vec::new();
    let mut new_offset = offset;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 || !line.ends_with('\n') {
            break;
        }
        new_offset += n as u64;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        out.push(parse_line(local_id, trimmed));
        if trimmed.contains("\"rate_limits\"")
            && let Some(limits) = serde_json::from_str::<Value>(trimmed)
                .ok()
                .and_then(|v| crate::adapters::codex::rate_limits::from_rollout_line(local_id, &v))
        {
            out.push(limits);
        }
    }
    Ok((out, new_offset))
}

/// Re-read `path` from a window BEHIND `anchor`, realigned to a line boundary
/// so parsing never starts mid-line. The caller must not persist any offset
/// from this: it re-reads already-seen lines and relies on the server's
/// content-hash dedup to drop the duplicates and surface only real gaps.
pub(super) fn reconcile_tail(
    path: &Path,
    local_id: &str,
    anchor: u64,
) -> std::io::Result<Vec<AdapterEvent>> {
    use std::io::{BufRead, BufReader, Seek, SeekFrom};

    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e),
    };
    let len = file.metadata()?.len();
    if len == 0 {
        return Ok(vec![]);
    }
    let start = anchor.min(len).saturating_sub(RECONCILE_BACKUP_BYTES);
    file.seek(SeekFrom::Start(start))?;
    let mut reader = BufReader::new(file);
    if start > 0 {
        let mut partial = String::new();
        reader.read_line(&mut partial)?;
        if !partial.ends_with('\n') {
            return Ok(vec![]);
        }
    }
    let mut out = Vec::new();
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 || !line.ends_with('\n') {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        out.push(parse_line(local_id, trimmed));
    }
    Ok(out)
}

fn parse_line(local_id: &str, line: &str) -> AdapterEvent {
    if let Ok(value) = serde_json::from_str::<Value>(line) {
        // `turn_context` rollout lines carry the model + reasoning effort the
        // session runs on. Surface them as a Status so discovered
        // (log-tailed) codex sessions render model/effort in the list, instead
        // of letting the line fall through as a meaningless "message".
        if value.get("type").and_then(Value::as_str) == Some("turn_context")
            && let Some(status) = turn_context_status(local_id, &value)
        {
            return status;
        }
        if let Some(usage) = token_usage_event(local_id, &value) {
            return usage;
        }
        // Heuristic: lines that look like tool calls.
        if value.get("tool").is_some()
            || value.get("function_call").is_some()
            || value.get("type").and_then(Value::as_str) == Some("tool_use")
        {
            return AdapterEvent::ToolUse { local_id: local_id.to_owned(), payload: value };
        }
        return AdapterEvent::Message {
            local_id: local_id.to_owned(),
            payload: value,
            turn_id: None,
        };
    }
    AdapterEvent::Message {
        local_id: local_id.to_owned(),
        payload: json!({"role": "assistant", "text": line}),
        turn_id: None,
    }
}

/// Extract model + reasoning effort from a `turn_context` rollout line and build
/// a `Status` event. The model lives at `payload.model`; effort at
/// `payload.collaboration_mode.settings.reasoning_effort` (newer codex) or a
/// top-level `payload.reasoning_effort` fallback — both may be null. Returns
/// `None` when neither is present so we don't emit an empty Status.
fn turn_context_status(local_id: &str, value: &Value) -> Option<AdapterEvent> {
    let p = value.get("payload")?;
    let str_at = |v: &Value, ptr: &str| {
        v.pointer(ptr).and_then(Value::as_str).map(str::to_owned).filter(|s| !s.is_empty())
    };
    let model = str_at(p, "/model");
    let effort = str_at(p, "/collaboration_mode/settings/reasoning_effort")
        .or_else(|| str_at(p, "/reasoning_effort"));
    if model.is_none() && effort.is_none() {
        return None;
    }
    Some(AdapterEvent::Status {
        local_id: local_id.to_owned(),
        tempo: None,
        state: None,
        detail: None,
        activity: None,
        name: None,
        intent: None,
        model,
        effort,
        permission_mode: None,
        children: vec![],
    })
}

/// Map a codex `event_msg`/`token_count` rollout line → [`AdapterEvent::TokenUsage`].
/// Codex writes one after every model response with
/// `info.last_token_usage` = that response's delta and `info.total_token_usage`
/// = the running session total. We emit the `last` delta so the server's
/// per-message SUM reconstructs the total, exactly like the app-server driver's
/// [`crate::adapters::codex::app_server`] `thread/tokenUsage/updated` mapping (`inputTokens`
/// includes the cached count, so subtract it for the non-cached/cached split
/// the claude + app-server adapters use).
///
/// `message_id` is derived from the line's own content — the timestamp plus the
/// strictly-monotonic cumulative total — so re-tailing the same rollout file
/// after a daemon restart re-emits identical ids and the server's
/// `ON CONFLICT (session_id, message_id) DO NOTHING` upsert refuses to
/// double-count. Returns `None` for non-token lines so `parse_line` continues.
fn token_usage_event(local_id: &str, value: &Value) -> Option<AdapterEvent> {
    if value.get("type").and_then(Value::as_str) != Some("event_msg") {
        return None;
    }
    let payload = value.get("payload")?;
    if payload.get("type").and_then(Value::as_str) != Some("token_count") {
        return None;
    }
    let last = payload.pointer("/info/last_token_usage");
    let g = |k: &str| last.and_then(|l| l.get(k)).and_then(Value::as_u64).unwrap_or(0);
    let cached = g("cached_input_tokens");
    let cumulative = payload
        .pointer("/info/total_token_usage/total_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let ts = value.get("timestamp").and_then(Value::as_str).unwrap_or("");
    Some(AdapterEvent::TokenUsage {
        local_id: local_id.to_owned(),
        message_id: format!("codex-tokens-{ts}-{cumulative}"),
        input_tokens: g("input_tokens").saturating_sub(cached),
        output_tokens: g("output_tokens"),
        cache_read_tokens: cached,
        cache_creation_tokens: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::super::{LogTail, LogTailConfig};
    use super::*;
    use std::collections::HashSet;
    use std::time::Duration;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    #[test]
    fn parse_line_handles_plain_text() {
        let evt = parse_line("s1", "hello world");
        assert!(matches!(evt, AdapterEvent::Message { .. }));
    }
    #[test]
    fn turn_context_line_emits_status_with_model_and_effort() {
        let line = r#"{"type":"turn_context","payload":{"model":"gpt-5.5","collaboration_mode":{"settings":{"reasoning_effort":"high"}}}}"#;
        match parse_line("s1", line) {
            AdapterEvent::Status { model, effort, .. } => {
                assert_eq!(model.as_deref(), Some("gpt-5.5"));
                assert_eq!(effort.as_deref(), Some("high"));
            }
            other => panic!("expected Status, got {other:?}"),
        }
    }
    #[test]
    fn turn_context_with_null_effort_still_surfaces_model() {
        let line = r#"{"type":"turn_context","payload":{"model":"gpt-5.5","collaboration_mode":{"settings":{"reasoning_effort":null}}}}"#;
        match parse_line("s1", line) {
            AdapterEvent::Status { model, effort, .. } => {
                assert_eq!(model.as_deref(), Some("gpt-5.5"));
                assert_eq!(effort, None);
            }
            other => panic!("expected Status, got {other:?}"),
        }
    }
    #[test]
    fn turn_context_without_model_or_effort_falls_through_to_message() {
        let line = r#"{"type":"turn_context","payload":{"cwd":"/w"}}"#;
        assert!(matches!(parse_line("s1", line), AdapterEvent::Message { .. }));
    }
    const ROLLOUT_FIXTURE: &str = include_str!("../fixtures/rollout_token_usage.jsonl");
    const HISTORY_FIXTURE: &str = include_str!("../fixtures/rollout_history.jsonl");
    #[test]
    fn uuid_from_stem_extracts_canonical_uuid() {
        let stem = "rollout-2026-07-12T01-25-55-019f51ff-f19f-7ed2-bf2a-bbb0d5cc5b90";
        assert_eq!(uuid_from_stem(stem).as_deref(), Some("019f51ff-f19f-7ed2-bf2a-bbb0d5cc5b90"));
        assert_eq!(uuid_from_stem("no-uuid-here"), None);
    }
    #[test]
    fn derive_local_id_prefers_session_meta_over_filename() {
        let tmp = tempfile::tempdir().unwrap();
        // Filename UUID differs from the session_meta id to prove meta wins.
        let path = tmp
            .path()
            .join("rollout-2026-07-12T01-25-55-ffffffff-0000-7000-8000-000000000000.jsonl");
        std::fs::write(&path, HISTORY_FIXTURE).unwrap();
        assert_eq!(derive_local_id(&path), "019f5200-aaaa-7bbb-8ccc-000000000001");
    }
    #[test]
    fn derive_local_id_falls_back_to_filename_uuid() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp
            .path()
            .join("rollout-2026-07-12T01-25-55-019f51ff-f19f-7ed2-bf2a-bbb0d5cc5b90.jsonl");
        std::fs::write(&path, "{\"role\":\"assistant\",\"text\":\"no meta\"}\n").unwrap();
        assert_eq!(derive_local_id(&path), "019f51ff-f19f-7ed2-bf2a-bbb0d5cc5b90");
    }
    #[tokio::test]
    async fn recursive_scan_finds_nested_rollout_with_canonical_id() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().to_path_buf();
        let nested = sessions.join("2026").join("07").join("12");
        std::fs::create_dir_all(&nested).unwrap();
        let path =
            nested.join("rollout-2026-07-12T01-25-55-ffffffff-0000-7000-8000-000000000000.jsonl");
        std::fs::write(&path, HISTORY_FIXTURE).unwrap();
        let (tx, mut rx) = mpsc::channel(64);
        let mut tail = LogTail::new(
            LogTailConfig {
                sessions_root: sessions,
                poll_interval: Duration::from_millis(10),
                quiesce: Duration::from_hours(1),
                offsets_path: None,
            },
            tx,
            CancellationToken::new(),
        );
        tail.scan_once().await;
        let started = rx.recv().await.unwrap();
        match started {
            AdapterEvent::SessionStarted { local_id, .. } => {
                assert_eq!(local_id, "019f5200-aaaa-7bbb-8ccc-000000000001");
            }
            other => panic!("expected SessionStarted, got {other:?}"),
        }
        // Every subsequent event must carry the canonical id, not the filename.
        let mut saw_message = false;
        while let Ok(evt) = rx.try_recv() {
            let id = match &evt {
                AdapterEvent::Message { local_id, .. }
                | AdapterEvent::ToolUse { local_id, .. }
                | AdapterEvent::Status { local_id, .. }
                | AdapterEvent::TokenUsage { local_id, .. }
                | AdapterEvent::TranscriptMark { local_id, .. } => local_id.clone(),
                other => panic!("unexpected event {other:?}"),
            };
            assert_eq!(id, "019f5200-aaaa-7bbb-8ccc-000000000001");
            if matches!(evt, AdapterEvent::Message { .. }) {
                saw_message = true;
            }
        }
        assert!(saw_message, "nested rollout transcript must be tailed");
    }
    #[test]
    fn history_fixture_response_and_event_envelopes_parse() {
        let events: Vec<AdapterEvent> = HISTORY_FIXTURE
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| parse_line("hist", l.trim()))
            .collect();
        // token_count → TokenUsage, turn_context → Status, the rest → Message.
        assert_eq!(
            events.iter().filter(|e| matches!(e, AdapterEvent::TokenUsage { .. })).count(),
            1
        );
        assert_eq!(events.iter().filter(|e| matches!(e, AdapterEvent::Status { .. })).count(), 1);
        // The response_item / event_msg envelopes are preserved verbatim as
        // Message payloads so the server-side normalizer can unwrap them.
        let has_envelope = |t: &str| {
            events.iter().any(|e| {
                matches!(e,
                AdapterEvent::Message { payload, .. }
                    if payload.get("type").and_then(Value::as_str) == Some(t))
            })
        };
        assert!(has_envelope("response_item"));
        assert!(has_envelope("event_msg"));
    }
    #[test]
    fn token_count_line_emits_token_usage() {
        let line = r#"{"timestamp":"2026-05-30T07:37:04.869Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":23695},"last_token_usage":{"input_tokens":11860,"cached_input_tokens":9600,"output_tokens":214,"reasoning_output_tokens":117,"total_tokens":12074}}}}"#;
        match parse_line("sess", line) {
            AdapterEvent::TokenUsage {
                local_id,
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_creation_tokens,
                ..
            } => {
                assert_eq!(local_id, "sess");
                assert_eq!(input_tokens, 11860 - 9600);
                assert_eq!(output_tokens, 214);
                assert_eq!(cache_read_tokens, 9600);
                assert_eq!(cache_creation_tokens, 0);
            }
            other => panic!("expected TokenUsage, got {other:?}"),
        }
    }
    #[test]
    fn token_usage_message_id_is_stable_and_distinct_per_line() {
        let a = r#"{"timestamp":"2026-05-30T07:36:59.740Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":11621},"last_token_usage":{"input_tokens":11111,"cached_input_tokens":9600,"output_tokens":510,"total_tokens":11621}}}}"#;
        let b = r#"{"timestamp":"2026-05-30T07:37:04.869Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":23695},"last_token_usage":{"input_tokens":11860,"cached_input_tokens":9600,"output_tokens":214,"total_tokens":12074}}}}"#;
        let id = |line: &str| match parse_line("s", line) {
            AdapterEvent::TokenUsage { message_id, .. } => message_id,
            other => panic!("expected TokenUsage, got {other:?}"),
        };
        assert_eq!(id(a), id(a));
        assert_ne!(id(a), id(b));
    }
    #[test]
    fn fixture_rollout_accumulates_per_turn_token_usage() {
        let events: Vec<AdapterEvent> = ROLLOUT_FIXTURE
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| parse_line("fixture", l.trim()))
            .collect();
        let usages: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                AdapterEvent::TokenUsage {
                    message_id,
                    input_tokens,
                    output_tokens,
                    cache_read_tokens,
                    ..
                } => Some((message_id.clone(), *input_tokens, *output_tokens, *cache_read_tokens)),
                _ => None,
            })
            .collect();
        assert_eq!(usages.len(), 3);
        let ids: HashSet<&String> = usages.iter().map(|(id, ..)| id).collect();
        assert_eq!(ids.len(), 3, "message ids must be unique per token_count line");
        let sum_in: u64 = usages.iter().map(|(_, i, ..)| i).sum();
        let sum_out: u64 = usages.iter().map(|(_, _, o, _)| o).sum();
        let sum_cache: u64 = usages.iter().map(|(.., c)| c).sum();
        assert_eq!(sum_in, (11111 - 9600) + (11860 - 9600) + (12134 - 10624));
        assert_eq!(sum_out, 510 + 214 + 61);
        assert_eq!(sum_cache, 9600 + 9600 + 10624);
        // token_count lines must NOT also surface as transcript messages.
        assert!(
            !events.iter().any(|e| matches!(
                e,
                AdapterEvent::Message { payload, .. }
                    if payload.pointer("/payload/type").and_then(Value::as_str) == Some("token_count")
            )),
            "token_count lines must map to TokenUsage, not Message"
        );
    }
}
