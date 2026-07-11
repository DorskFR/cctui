//! Codex log-tail adapter (CCT-89).
//!
//! Watches `~/.codex/sessions/` for new log files. For each new file:
//!
//! 1. Emit `SessionStarted` with `local_id` = file basename (without
//!    `.jsonl` / `.log` suffix) and `working_dir` from the
//!    `cwd`/`working_dir` field in the first parseable JSON line that
//!    carries one (if any).
//! 2. Tail subsequent lines: if the line parses as JSON it becomes a
//!    `Message` payload as-is; otherwise it's wrapped as
//!    `{role: "assistant", text: <line>}`. Tool-call payloads are
//!    recognised heuristically by the presence of a `"tool"` or
//!    `"function_call"` field.
//! 3. After `quiesce_secs` of no new bytes on a tracked file, emit
//!    `SessionEnded { Completed }` and drop the tracking entry.
//!
//! The exact Codex log schema isn't documented here — this is an
//! opt-in scaffold that will need refinement once we have concrete
//! fixtures. The line parser is intentionally permissive.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cctui_proto::adapter::{AdapterEvent, EndReason, SessionMeta};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub struct LogTailConfig {
    pub sessions_root: PathBuf,
    pub poll_interval: Duration,
    pub quiesce: Duration,
}

impl Default for LogTailConfig {
    fn default() -> Self {
        Self {
            sessions_root: default_sessions_root(),
            poll_interval: Duration::from_secs(2),
            quiesce: Duration::from_secs(60),
        }
    }
}

impl LogTailConfig {
    pub fn from_value(v: &Value) -> Self {
        let mut cfg = Self::default();
        if let Some(p) = v.get("sessions_root").and_then(Value::as_str) {
            cfg.sessions_root = PathBuf::from(p);
        }
        if let Some(ms) = v.get("poll_interval_ms").and_then(Value::as_u64) {
            cfg.poll_interval = Duration::from_millis(ms);
        }
        if let Some(s) = v.get("quiesce_secs").and_then(Value::as_u64) {
            cfg.quiesce = Duration::from_secs(s);
        }
        cfg
    }
}

#[must_use]
pub fn default_sessions_root() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")).join(".codex").join("sessions")
}

#[derive(Debug)]
struct TrackedSession {
    local_id: String,
    offset: u64,
    last_activity: Instant,
}

pub struct LogTail {
    cfg: LogTailConfig,
    events: mpsc::Sender<AdapterEvent>,
    shutdown: CancellationToken,
    sessions: HashMap<PathBuf, TrackedSession>,
    /// Sessions driven by the app-server (CCT-98). Their rollout files are
    /// skipped here so we don't double-ingest. `local_id` is the rollout
    /// `UUIDv7`, which is a suffix of the rollout filename stem.
    owned: Option<super::app_server::SessionRegistry>,
}

impl LogTail {
    pub fn new(
        cfg: LogTailConfig,
        events: mpsc::Sender<AdapterEvent>,
        shutdown: CancellationToken,
    ) -> Self {
        Self { cfg, events, shutdown, sessions: HashMap::new(), owned: None }
    }

    /// Share the app-server session registry so app-server-owned rollout
    /// files are skipped (no double-ingest of the same session).
    pub fn set_owned(&mut self, registry: super::app_server::SessionRegistry) {
        self.owned = Some(registry);
    }

    pub async fn run(mut self) -> anyhow::Result<()> {
        let mut tick = tokio::time::interval(self.cfg.poll_interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                () = self.shutdown.cancelled() => return Ok(()),
                _ = tick.tick() => {
                    self.scan_once().await;
                }
            }
        }
    }

    async fn scan_once(&mut self) {
        let Ok(entries) = std::fs::read_dir(&self.cfg.sessions_root) else { return };
        // App-server-owned session ids (rollout UUIDv7). Files whose stem
        // ends with one of these are driven directly via app-server and must
        // not be tailed here.
        let owned: Vec<String> = match &self.owned {
            Some(reg) => reg.lock().await.keys().cloned().collect(),
            None => Vec::new(),
        };
        // NOTE (CCT-276): we deliberately do NOT skip files for ids the
        // `thread/list` inventory has surfaced. The inventory only seeds a
        // single preview message; the real transcript lives in the rollout
        // JSONL. Suppressing the tail left discovered CLI sessions with an
        // empty conversation ("No events yet"). The app-server `owned` set
        // above is still skipped — those threads are driven live by cctui.
        let mut alive: HashSet<PathBuf> = HashSet::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            if owned.iter().any(|id| stem.ends_with(id.as_str())) {
                continue;
            }
            alive.insert(path.clone());
            self.tail_file(path).await;
        }
        // Quiesce check: anything tracked but not seen this scan OR not
        // updated for `quiesce` → emit SessionEnded.
        let now = Instant::now();
        let ended: Vec<PathBuf> = self
            .sessions
            .iter()
            .filter(|(p, s)| {
                !alive.contains(*p) || now.duration_since(s.last_activity) > self.cfg.quiesce
            })
            .map(|(p, _)| p.clone())
            .collect();
        for path in ended {
            if let Some(s) = self.sessions.remove(&path) {
                let _ = self
                    .events
                    .send(AdapterEvent::SessionEnded {
                        local_id: s.local_id,
                        reason: EndReason::Completed,
                    })
                    .await;
            }
        }
    }

    async fn tail_file(&mut self, path: PathBuf) {
        let local_id = path.file_stem().and_then(|s| s.to_str()).unwrap_or("unknown").to_owned();

        let is_new = !self.sessions.contains_key(&path);
        let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if is_new {
            // Emit SessionStarted with no cwd yet; we'll update once we
            // see a line that carries one. Working dir refinement can
            // happen in a follow-up.
            let _ = self
                .events
                .send(AdapterEvent::SessionStarted {
                    local_id: local_id.clone(),
                    meta: SessionMeta {
                        working_dir: None,
                        extra: json!({"source": "codex-log-tail"}),
                        ..SessionMeta::default()
                    },
                })
                .await;
            self.sessions.insert(
                path.clone(),
                TrackedSession {
                    local_id: local_id.clone(),
                    offset: 0,
                    last_activity: Instant::now(),
                },
            );
        }

        let session = self.sessions.get_mut(&path).expect("inserted above");
        if len <= session.offset {
            return; // no new bytes
        }
        let events = match read_new_lines(&path, session.offset, &session.local_id) {
            Ok(res) => res,
            Err(err) => {
                tracing::debug!(%err, ?path, "codex log read failed");
                return;
            }
        };
        session.offset = len;
        if !events.is_empty() {
            session.last_activity = Instant::now();
        }
        for evt in events {
            let _ = self.events.send(evt).await;
        }
    }
}

fn read_new_lines(path: &Path, offset: u64, local_id: &str) -> std::io::Result<Vec<AdapterEvent>> {
    use std::io::{BufRead, BufReader, Seek, SeekFrom};

    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    if len <= offset {
        return Ok(vec![]);
    }
    file.seek(SeekFrom::Start(offset))?;
    let reader = BufReader::new(file);
    let mut out = Vec::new();
    for line in reader.lines() {
        let line = line?;
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
        // session runs on (CCT-299). Surface them as a Status so discovered
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
        return AdapterEvent::Message { local_id: local_id.to_owned(), payload: value };
    }
    AdapterEvent::Message {
        local_id: local_id.to_owned(),
        payload: json!({"role": "assistant", "text": line}),
    }
}

/// Extract model + reasoning effort from a `turn_context` rollout line and build
/// a `Status` event (CCT-299). The model lives at `payload.model`; effort at
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
        children: vec![],
    })
}

/// Map a codex `event_msg`/`token_count` rollout line → [`AdapterEvent::TokenUsage`]
/// (CCT-597). Codex writes one after every model response with
/// `info.last_token_usage` = that response's delta and `info.total_token_usage`
/// = the running session total. We emit the `last` delta so the server's
/// per-message SUM reconstructs the total, exactly like the app-server driver's
/// [`super::app_server`] `thread/tokenUsage/updated` mapping (`inputTokens`
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
    use super::*;
    use std::io::Write;

    #[tokio::test]
    async fn detects_new_session_file() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().to_path_buf();
        let (tx, mut rx) = mpsc::channel(64);
        let mut tail = LogTail::new(
            LogTailConfig {
                sessions_root: sessions.clone(),
                poll_interval: Duration::from_millis(10),
                quiesce: Duration::from_secs(3600),
            },
            tx,
            CancellationToken::new(),
        );
        let path = sessions.join("session-abc.jsonl");
        std::fs::write(&path, r#"{"role":"assistant","text":"hi"}"#).unwrap();
        tail.scan_once().await;
        // Started + Message.
        let evt1 = rx.recv().await.unwrap();
        let evt2 = rx.recv().await.unwrap();
        assert!(matches!(evt1, AdapterEvent::SessionStarted { .. }));
        assert!(matches!(evt2, AdapterEvent::Message { .. }));
    }

    #[tokio::test]
    async fn quiesce_emits_session_ended() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().to_path_buf();
        let (tx, mut rx) = mpsc::channel(64);
        let mut tail = LogTail::new(
            LogTailConfig {
                sessions_root: sessions.clone(),
                poll_interval: Duration::from_millis(10),
                quiesce: Duration::from_millis(1),
            },
            tx,
            CancellationToken::new(),
        );
        let path = sessions.join("s1.jsonl");
        std::fs::write(&path, r#"{"role":"assistant","text":"hi"}"#).unwrap();
        tail.scan_once().await;
        // Drain Started + Message.
        rx.recv().await.unwrap();
        rx.recv().await.unwrap();
        // Wait past quiesce window, then scan again.
        tokio::time::sleep(Duration::from_millis(20)).await;
        tail.scan_once().await;
        let evt = rx.recv().await.unwrap();
        assert!(matches!(evt, AdapterEvent::SessionEnded { .. }));
    }

    #[tokio::test]
    async fn tool_lines_emit_tool_use() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().to_path_buf();
        let (tx, mut rx) = mpsc::channel(64);
        let mut tail = LogTail::new(
            LogTailConfig {
                sessions_root: sessions.clone(),
                poll_interval: Duration::from_millis(10),
                quiesce: Duration::from_secs(3600),
            },
            tx,
            CancellationToken::new(),
        );
        let path = sessions.join("s1.jsonl");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, r#"{{"tool":"shell","args":["ls"]}}"#).unwrap();
        tail.scan_once().await;
        rx.recv().await.unwrap(); // started
        let evt = rx.recv().await.unwrap();
        assert!(matches!(evt, AdapterEvent::ToolUse { .. }));
    }

    #[tokio::test]
    async fn inventory_discovered_session_still_tails_transcript() {
        // CCT-276 regression: a session whose rollout id was discovered by the
        // thread/list inventory must still get its real JSONL transcript tailed
        // here. Before the fix the log-tail skipped files whose stem matched an
        // inventory id, leaving the conversation empty ("No events yet").
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().to_path_buf();
        let (tx, mut rx) = mpsc::channel(64);
        let mut tail = LogTail::new(
            LogTailConfig {
                sessions_root: sessions.clone(),
                poll_interval: Duration::from_millis(10),
                quiesce: Duration::from_secs(3600),
            },
            tx,
            CancellationToken::new(),
        );
        // Rollout filename whose stem ends with the inventory-discovered id.
        let id = "019ea66a-cf6e-73b1";
        let path = sessions.join(format!("rollout-2026-{id}.jsonl"));
        std::fs::write(&path, r#"{"role":"assistant","text":"real transcript"}"#).unwrap();
        tail.scan_once().await;
        let evt1 = rx.recv().await.unwrap();
        let evt2 = rx.recv().await.unwrap();
        assert!(matches!(evt1, AdapterEvent::SessionStarted { .. }));
        assert!(matches!(evt2, AdapterEvent::Message { .. }), "transcript must be tailed");
    }

    #[tokio::test]
    async fn app_server_owned_session_is_skipped() {
        // The app-server `owned` set is still honored — cctui drives those live.
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().to_path_buf();
        let (tx, mut rx) = mpsc::channel(64);
        let mut tail = LogTail::new(
            LogTailConfig {
                sessions_root: sessions.clone(),
                poll_interval: Duration::from_millis(10),
                quiesce: Duration::from_secs(3600),
            },
            tx,
            CancellationToken::new(),
        );
        let registry = super::super::app_server::SessionRegistry::default();
        let id = "owned-019ea66a";
        registry.lock().await.insert(
            id.to_owned(),
            super::super::app_server::SessionRecord {
                cfg: super::super::app_server::AppServerConfig::default(),
                cwd: "/w".into(),
                name: None,
                env: std::collections::BTreeMap::new(),
            },
        );
        tail.set_owned(registry);
        let path = sessions.join(format!("rollout-{id}.jsonl"));
        std::fs::write(&path, r#"{"role":"assistant","text":"x"}"#).unwrap();
        tail.scan_once().await;
        assert!(rx.try_recv().is_err(), "owned rollout file must not be tailed");
    }

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

    const ROLLOUT_FIXTURE: &str = include_str!("fixtures/rollout_token_usage.jsonl");

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
