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
    /// Ids surfaced by the `thread/list` inventory (CCT-263). Those carry
    /// richer metadata (preview/name/cwd/status) than the heuristic scrape, so
    /// the log-tail skips their files to avoid a second, poorer `SessionStarted`
    /// for the same session.
    inventory: Option<super::thread_list::SeenIds>,
}

impl LogTail {
    pub fn new(
        cfg: LogTailConfig,
        events: mpsc::Sender<AdapterEvent>,
        shutdown: CancellationToken,
    ) -> Self {
        Self { cfg, events, shutdown, sessions: HashMap::new(), owned: None, inventory: None }
    }

    /// Share the app-server session registry so app-server-owned rollout
    /// files are skipped (no double-ingest of the same session).
    pub fn set_owned(&mut self, registry: super::app_server::SessionRegistry) {
        self.owned = Some(registry);
    }

    /// Share the `thread/list` inventory's surfaced-id set so those rollout
    /// files are skipped here (CCT-263 — the inventory is the richer source).
    pub fn set_inventory(&mut self, seen: super::thread_list::SeenIds) {
        self.inventory = Some(seen);
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
        let mut owned: Vec<String> = match &self.owned {
            Some(reg) => reg.lock().await.keys().cloned().collect(),
            None => Vec::new(),
        };
        // Also skip files for sessions the thread/list inventory has surfaced.
        if let Some(seen) = &self.inventory {
            owned.extend(seen.lock().await.keys().cloned());
        }
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

    #[test]
    fn parse_line_handles_plain_text() {
        let evt = parse_line("s1", "hello world");
        assert!(matches!(evt, AdapterEvent::Message { .. }));
    }
}
