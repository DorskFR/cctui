//! Codex log-tail adapter.
//!
//! Watches `~/.codex/sessions/`. A new file emits `SessionStarted` (`local_id`
//! is the basename, `working_dir` the first `cwd`/`working_dir` seen); each
//! line becomes a `Message` (non-JSON is wrapped as assistant text, and a
//! `"tool"`/`"function_call"` field marks a tool call). After `quiesce_secs`
//! without growth the session is `hibernated`, not ended: it resumes when the
//! file grows. `SessionEnded` is reserved for the file disappearing.
//!
//! The Codex log schema is undocumented, so the line parser is permissive.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use cctui_proto::adapter::{AdapterEvent, EndReason, SessionMeta};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

mod rollout;

use rollout::{
    collect_rollout_files, derive_local_id, read_new_lines, reconcile_tail, rollout_link,
};

#[derive(Debug, Clone)]
pub struct LogTailConfig {
    pub sessions_root: PathBuf,
    pub poll_interval: Duration,
    pub quiesce: Duration,
    pub offsets_path: Option<PathBuf>,
}

impl Default for LogTailConfig {
    fn default() -> Self {
        Self {
            sessions_root: default_sessions_root(),
            poll_interval: Duration::from_secs(2),
            quiesce: Duration::from_mins(1),
            offsets_path: dirs::config_dir().map(|d| d.join("cctui").join("codex-offsets.json")),
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
    hibernated: bool,
}

pub struct LogTail {
    cfg: LogTailConfig,
    events: mpsc::Sender<AdapterEvent>,
    shutdown: CancellationToken,
    sessions: HashMap<PathBuf, TrackedSession>,
    /// Sessions driven by the app-server. Their rollout files are
    /// skipped here so we don't double-ingest. `local_id` is the rollout
    /// `UUIDv7`, which is a suffix of the rollout filename stem.
    owned: Option<super::app_server::SessionRegistry>,
    /// Threads whose transcript was served structurally via
    /// `thread/read` + `thread/turns/list`. Their rollout files are skipped
    /// for the same no-double-ingest reason as `owned`.
    served: Option<super::thread_read::ServedIds>,
    /// Rollout-path → byte offset, persisted so restarts and quiesce
    /// evictions never re-read (re-upload) historical rollouts.
    offsets: crate::offsets::OffsetStore,
    offsets_dirty: bool,
    /// `local_id` → offset the server last persisted. A mark behind our own
    /// offset means events were emitted but never stored, so the reconcile
    /// pass replays from the mark instead.
    marks: ResumeMarks,
    reconciled: HashSet<PathBuf>,
    index: RolloutIndex,
    /// Derived `local_id` of quiet untracked rollouts, so the per-tick mark
    /// check never re-opens them.
    quiet_ids: HashMap<PathBuf, String>,
}

/// How often the whole sessions tree is re-walked; fast ticks only list the
/// recent date dirs and stat the rollouts already being tailed.
const FULL_WALK_EVERY: Duration = Duration::from_mins(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileStat {
    len: u64,
    mtime: Option<SystemTime>,
}

/// Known rollout files with their last observed `(len, mtime)`.
#[derive(Debug, Default)]
struct RolloutIndex {
    files: HashMap<PathBuf, FileStat>,
    last_full: Option<Instant>,
    stats: usize,
}

impl RolloutIndex {
    fn full_due(&self, now: Instant) -> bool {
        self.last_full.is_none_or(|t| now.duration_since(t) >= FULL_WALK_EVERY)
    }

    /// Blocking: must run under `spawn_blocking`.
    fn refresh(&mut self, root: &Path, tracked: &[PathBuf], full: bool, now: Instant) {
        self.stats = 0;
        if full {
            let mut files = Vec::new();
            collect_rollout_files(root, 0, &mut files);
            self.files.clear();
            for path in files {
                self.stat_into(path);
            }
            self.last_full = Some(now);
            return;
        }
        let mut listed = HashSet::new();
        for dir in recent_dirs(root) {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for entry in entries.flatten() {
                if entry.file_type().is_ok_and(|t| t.is_file()) {
                    listed.insert(entry.path());
                }
            }
            self.files.retain(|p, _| p.parent() != Some(dir.as_path()) || listed.contains(p));
        }
        listed.extend(tracked.iter().cloned());
        for path in listed {
            self.stat_into(path);
        }
    }

    fn stat_into(&mut self, path: PathBuf) {
        self.stats += 1;
        match std::fs::metadata(&path) {
            Ok(m) if m.is_file() => {
                self.files.insert(path, FileStat { len: m.len(), mtime: m.modified().ok() });
            }
            _ => {
                self.files.remove(&path);
            }
        }
    }
}

/// The root itself (flat layout) plus `YYYY/MM/DD` for today and yesterday,
/// in both local time and UTC so a midnight rollover in either is covered.
fn recent_dirs(root: &Path) -> Vec<PathBuf> {
    let now_local = chrono::Local::now().date_naive();
    let now_utc = chrono::Utc::now().date_naive();
    let mut dirs = vec![root.to_path_buf()];
    for day in [now_local, now_utc] {
        for d in [Some(day), day.pred_opt()].into_iter().flatten() {
            let dir = root.join(d.format("%Y/%m/%d").to_string());
            if !dirs.contains(&dir) {
                dirs.push(dir);
            }
        }
    }
    dirs
}

/// The server's per-session transcript marks, written by the codex command
/// pump on a `ResumeMarks` frame and read by the tail when it adopts a rollout.
pub type ResumeMarks = Arc<Mutex<HashMap<String, u64>>>;

/// How far behind the persisted offset the reconcile pass backs up before
/// re-reading, mirroring the claude-code transcript tailer. The server's
/// `(session_id, event_type, content_hash, turn_id)` dedup drops every replayed
/// duplicate, so the window can be generous.
pub const RECONCILE_BACKUP_BYTES: u64 = 64 * 1024;

impl LogTail {
    pub fn new(
        cfg: LogTailConfig,
        events: mpsc::Sender<AdapterEvent>,
        shutdown: CancellationToken,
    ) -> Self {
        let offsets = crate::offsets::OffsetStore::open(cfg.offsets_path.clone());
        Self {
            cfg,
            events,
            shutdown,
            sessions: HashMap::new(),
            owned: None,
            served: None,
            offsets,
            offsets_dirty: false,
            marks: ResumeMarks::default(),
            reconciled: HashSet::new(),
            index: RolloutIndex::default(),
            quiet_ids: HashMap::new(),
        }
    }

    /// Share the store the command pump writes server transcript marks into.
    pub fn set_resume_marks(&mut self, marks: ResumeMarks) {
        self.marks = marks;
    }

    /// Share the app-server session registry so app-server-owned rollout
    /// files are skipped (no double-ingest of the same session).
    pub fn set_owned(&mut self, registry: super::app_server::SessionRegistry) {
        self.owned = Some(registry);
    }

    /// Share the set of threads the structured history reader has already
    /// served, so the heuristic tail stays a fallback rather than a duplicate.
    pub fn set_served(&mut self, served: super::thread_read::ServedIds) {
        self.served = Some(served);
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
        // App-server-owned session ids (rollout UUIDv7). Files whose stem
        // ends with one of these are driven directly via app-server and must
        // not be tailed here.
        let mut owned: Vec<String> = match &self.owned {
            Some(reg) => reg.lock().await.keys().cloned().collect(),
            None => Vec::new(),
        };
        if let Some(served) = &self.served {
            owned.extend(served.lock().await.iter().cloned());
        }
        // Ids merely surfaced by the `thread/list` inventory are NOT skipped:
        // the inventory alone seeds only a preview, and suppressing the tail
        // left discovered CLI sessions with an empty conversation. Only
        // threads cctui drives live (`owned`) or whose real transcript came
        // back from `thread/turns/list` (`served`) are skipped.
        let mut alive: HashSet<PathBuf> = HashSet::new();
        self.refresh_index().await;
        let mut files: Vec<(PathBuf, FileStat)> =
            self.index.files.iter().map(|(p, st)| (p.clone(), *st)).collect();
        files.sort_by(|a, b| a.0.cmp(&b.0));
        for (path, stat) in files {
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            if owned.iter().any(|id| stem.ends_with(id.as_str())) {
                continue;
            }
            alive.insert(path.clone());
            self.tail_file(path, stat).await;
        }
        // The rollout is gone: the only evidence of an end this adapter has.
        let ended: Vec<PathBuf> =
            self.sessions.keys().filter(|p| !alive.contains(*p)).cloned().collect();
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
        let now = Instant::now();
        let idle: Vec<PathBuf> = self
            .sessions
            .iter()
            .filter(|(_, s)| {
                !s.hibernated && now.duration_since(s.last_activity) > self.cfg.quiesce
            })
            .map(|(p, _)| p.clone())
            .collect();
        for path in idle {
            let Some(s) = self.sessions.get_mut(&path) else { continue };
            s.hibernated = true;
            let local_id = s.local_id.clone();
            let _ = self.events.send(hibernated_status(local_id)).await;
        }
        if !alive.is_empty() {
            let keep: HashSet<String> =
                alive.iter().map(|p| p.to_string_lossy().into_owned()).collect();
            if self.offsets.retain(|k| keep.contains(k)) {
                self.offsets_dirty = true;
            }
        }
        if self.offsets_dirty {
            let offsets = std::mem::take(&mut self.offsets);
            self.offsets = tokio::task::spawn_blocking(move || {
                offsets.flush();
                offsets
            })
            .await
            .unwrap_or_default();
            self.offsets_dirty = false;
        }
    }

    async fn refresh_index(&mut self) {
        let now = Instant::now();
        let full = self.index.full_due(now);
        let root = self.cfg.sessions_root.clone();
        let tracked: Vec<PathBuf> = self.sessions.keys().cloned().collect();
        let mut index = std::mem::take(&mut self.index);
        self.index = tokio::task::spawn_blocking(move || {
            index.refresh(&root, &tracked, full, now);
            index
        })
        .await
        .unwrap_or_default();
    }

    async fn tail_file(&mut self, path: PathBuf, stat: FileStat) {
        let len = stat.len;
        let key = path.to_string_lossy().into_owned();

        let is_new = !self.sessions.contains_key(&path);
        if is_new {
            // Quiet rollout with nothing beyond the persisted offset: leave it
            // untracked so it stays invisible (no Started/Ended churn) unless
            // the server still holds a mark for it — then the gap behind that
            // mark is exactly what the reconcile pass must replay.
            if len <= self.offsets.get(&key) && !self.needs_quiet_reconcile(&path, &key).await {
                return;
            }
            self.quiet_ids.remove(&path);
            let observed_at = stat
                .mtime
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
            let p = path.clone();
            let Ok((local_id, link)) =
                tokio::task::spawn_blocking(move || (derive_local_id(&p), rollout_link(&p))).await
            else {
                return;
            };
            let mut extra = json!({"source": "codex-log-tail", "observed_at": observed_at});
            let mut parent_local_id = None;
            if link.source.as_deref().is_some_and(|s| s.starts_with("subAgent")) {
                // A subagent rollout that can't resolve a parent can never nest;
                // skip it so this path matches the inventory's orphan skip.
                let Some(parent) = link.subagent_parent else { return };
                parent_local_id =
                    Some(crate::dispatch_codex::dispatch_session_for(&parent).unwrap_or(parent));
                extra["subagent"] = json!(true);
            } else if let Some(parent) = link.launcher_parent
                // A self-parent would make the server's recursive heartbeat CTE
                // walk a cycle, so refuse it.
                && parent != local_id
            {
                parent_local_id = Some(parent);
                extra["subagent"] = json!(true);
            }
            let _ = self
                .events
                .send(AdapterEvent::SessionStarted {
                    local_id: local_id.clone(),
                    meta: SessionMeta { working_dir: None, parent_local_id, extra },
                })
                .await;
            self.sessions.insert(
                path.clone(),
                TrackedSession {
                    local_id,
                    offset: self.offsets.get(&key).min(len),
                    last_activity: Instant::now(),
                    hibernated: false,
                },
            );
        }

        self.reconcile_once(&path, &key).await;

        let session = self.sessions.get(&path).expect("inserted above");
        let (offset, local_id) = (session.offset, session.local_id.clone());
        if len <= offset {
            return; // no new bytes
        }
        let (p, id) = (path.clone(), local_id.clone());
        let read = tokio::task::spawn_blocking(move || read_new_lines(&p, offset, &id))
            .await
            .unwrap_or_else(|err| Err(std::io::Error::other(err)));
        let (events, new_offset) = match read {
            Ok(res) => res,
            Err(err) => {
                tracing::debug!(%err, ?path, "codex log read failed");
                return;
            }
        };
        if events.is_empty() {
            let session = self.sessions.get_mut(&path).expect("inserted above");
            session.offset = new_offset;
            if new_offset > offset {
                self.offsets.set(key, new_offset);
                self.offsets_dirty = true;
            }
            return;
        }
        // The offset may only advance over events the receiver actually took:
        // a dropped send is a hole the next scan has to re-read, and a
        // persisted offset past it would make that hole permanent.
        for evt in events {
            if self.events.send(evt).await.is_err() {
                return;
            }
        }
        let session = self.sessions.get_mut(&path).expect("inserted above");
        session.offset = new_offset;
        session.last_activity = Instant::now();
        session.hibernated = false;
        self.offsets.set(key, new_offset);
        self.offsets_dirty = true;
        let _ =
            self.events.send(AdapterEvent::TranscriptMark { local_id, offset: new_offset }).await;
    }

    /// A quiet rollout still worth adopting: one we have tailed before and the
    /// server holds a mark for, i.e. a candidate for a gap that opened while
    /// its events were going nowhere.
    async fn needs_quiet_reconcile(&mut self, path: &Path, key: &str) -> bool {
        if self.reconciled.contains(path) || self.offsets.get(key) == 0 {
            return false;
        }
        if !self.marks.lock().is_ok_and(|m| !m.is_empty()) {
            return false;
        }
        let local_id = if let Some(id) = self.quiet_ids.get(path) {
            id.clone()
        } else {
            let p = path.to_path_buf();
            let Ok(id) = tokio::task::spawn_blocking(move || derive_local_id(&p)).await else {
                return false;
            };
            self.quiet_ids.insert(path.to_path_buf(), id.clone());
            id
        };
        self.marks.lock().is_ok_and(|m| m.contains_key(&local_id))
    }

    /// Bounded re-read behind the persisted offset, once per rollout per
    /// process. Emits without advancing any offset: it deliberately re-reads
    /// seen lines and leans on the server's content-hash dedup.
    async fn reconcile_once(&mut self, path: &Path, key: &str) {
        if !self.reconciled.insert(path.to_path_buf()) {
            return;
        }
        let persisted = self.offsets.get(key);
        if persisted == 0 {
            return;
        }
        let Some(session) = self.sessions.get(path) else { return };
        let local_id = session.local_id.clone();
        let mark = self.marks.lock().ok().and_then(|m| m.get(&local_id).copied());
        let anchor = mark.map_or(persisted, |m| m.min(persisted));
        let (p, id) = (path.to_path_buf(), local_id.clone());
        let read = tokio::task::spawn_blocking(move || reconcile_tail(&p, &id, anchor))
            .await
            .unwrap_or_else(|err| Err(std::io::Error::other(err)));
        let events = match read {
            Ok(events) => events,
            Err(err) => {
                tracing::debug!(%err, ?path, "codex reconcile read failed");
                return;
            }
        };
        if events.is_empty() {
            return;
        }
        tracing::info!(%local_id, anchor, count = events.len(), "codex: reconciling rollout tail");
        for evt in events {
            if self.events.send(evt).await.is_err() {
                return;
            }
        }
    }
}

fn hibernated_status(local_id: String) -> AdapterEvent {
    AdapterEvent::Status {
        local_id,
        tempo: Some("hibernated".to_owned()),
        state: None,
        detail: None,
        activity: None,
        name: None,
        intent: None,
        model: None,
        effort: None,
        permission_mode: None,
        children: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const ROLLOUT: &str = "rollout-2026-09-07T01-00-00-019f51ff-f19f-7ed2-bf2a-bbb0d5cc5b90";
    const ROLLOUT_ID: &str = "019f51ff-f19f-7ed2-bf2a-bbb0d5cc5b90";
    fn write_turns(path: &Path, range: std::ops::Range<usize>) {
        let mut f =
            std::fs::OpenOptions::new().create(true).append(true).open(path).expect("open rollout");
        for i in range {
            writeln!(f, r#"{{"role":"assistant","text":"turn {i}"}}"#).unwrap();
        }
    }
    fn texts(events: &[AdapterEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|e| match e {
                AdapterEvent::Message { payload, .. } => {
                    payload.get("text").and_then(Value::as_str).map(str::to_owned)
                }
                _ => None,
            })
            .collect()
    }
    fn drain(rx: &mut mpsc::Receiver<AdapterEvent>) -> Vec<AdapterEvent> {
        let mut out = Vec::new();
        while let Ok(evt) = rx.try_recv() {
            out.push(evt);
        }
        out
    }
    fn tail_with(
        sessions: &Path,
        offsets_path: Option<PathBuf>,
        tx: mpsc::Sender<AdapterEvent>,
    ) -> LogTail {
        LogTail::new(
            LogTailConfig {
                sessions_root: sessions.to_path_buf(),
                poll_interval: Duration::from_millis(10),
                quiesce: Duration::from_hours(1),
                offsets_path,
            },
            tx,
            CancellationToken::new(),
        )
    }
    /// A daemon that kept tailing while its events went nowhere leaves the
    /// persisted offset past turns the server never stored. On restart the
    /// server's resume mark is the only evidence of where its copy stops, so
    /// the reconcile pass must replay from there — even though the rollout has
    /// not grown since.
    #[tokio::test]
    async fn resume_mark_replays_the_gap_a_dropped_connection_left() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().to_path_buf();
        let path = sessions.join(format!("{ROLLOUT}.jsonl"));
        let offsets_path = tmp.path().join("offsets.json");

        let (tx, mut rx) = mpsc::channel(64);
        let mut tail = tail_with(&sessions, Some(offsets_path.clone()), tx);
        write_turns(&path, 0..2);
        tail.scan_once().await;
        let stored = drain(&mut rx);
        assert_eq!(texts(&stored), ["turn 0", "turn 1"]);
        let mark = match stored.last().expect("events") {
            AdapterEvent::TranscriptMark { offset, .. } => *offset,
            other => panic!("expected a transcript mark, got {other:?}"),
        };

        // the WS is down: these turns are tailed but never reach the server,
        // and the offset is flushed past them anyway.
        write_turns(&path, 2..4);
        tail.scan_once().await;
        drop(tail);
        drop(rx);

        let (tx, mut rx) = mpsc::channel(64);
        let mut tail = tail_with(&sessions, Some(offsets_path), tx);
        tail.set_resume_marks(Arc::new(Mutex::new(HashMap::from([(ROLLOUT_ID.to_owned(), mark)]))));
        tail.scan_once().await;
        let healed = texts(&drain(&mut rx));
        assert!(
            healed.contains(&"turn 2".to_owned()) && healed.contains(&"turn 3".to_owned()),
            "the gap behind the mark must be replayed, got {healed:?}"
        );
    }
    /// The offset is a promise that everything before it was handed off. A
    /// failed send must leave it where it was so the next scan re-reads.
    #[tokio::test]
    async fn offset_does_not_advance_past_events_that_were_not_sent() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().to_path_buf();
        let path = sessions.join(format!("{ROLLOUT}.jsonl"));
        let offsets_path = tmp.path().join("offsets.json");
        write_turns(&path, 0..3);

        let (tx, rx) = mpsc::channel(64);
        drop(rx);
        let mut tail = tail_with(&sessions, Some(offsets_path), tx);
        tail.scan_once().await;
        let key = path.to_string_lossy().into_owned();
        assert_eq!(tail.offsets.get(&key), 0, "a dropped send must not advance the offset");

        let (tx, mut rx) = mpsc::channel(64);
        let mut tail = tail_with(&sessions, tail.cfg.offsets_path.clone(), tx);
        tail.scan_once().await;
        assert_eq!(texts(&drain(&mut rx)), ["turn 0", "turn 1", "turn 2"]);
    }
    #[tokio::test]
    async fn a_partial_trailing_line_is_re_read_whole() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().to_path_buf();
        let path = sessions.join(format!("{ROLLOUT}.jsonl"));
        let (tx, mut rx) = mpsc::channel(64);
        let mut tail = tail_with(&sessions, None, tx);

        std::fs::write(&path, "{\"role\":\"assistant\",\"text\":\"turn 0\"}\n{\"role\":\"assis")
            .unwrap();
        tail.scan_once().await;
        assert_eq!(texts(&drain(&mut rx)), ["turn 0"]);

        std::fs::write(
            &path,
            "{\"role\":\"assistant\",\"text\":\"turn 0\"}\n{\"role\":\"assistant\",\"text\":\"turn 1\"}\n",
        )
        .unwrap();
        tail.scan_once().await;
        assert_eq!(texts(&drain(&mut rx)), ["turn 1"]);
    }
    #[tokio::test]
    async fn detects_new_session_file() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().to_path_buf();
        let (tx, mut rx) = mpsc::channel(64);
        let mut tail = LogTail::new(
            LogTailConfig {
                sessions_root: sessions.clone(),
                poll_interval: Duration::from_millis(10),
                quiesce: Duration::from_hours(1),
                offsets_path: None,
            },
            tx,
            CancellationToken::new(),
        );
        let path = sessions.join("session-abc.jsonl");
        std::fs::write(&path, "{\"role\":\"assistant\",\"text\":\"hi\"}\n").unwrap();
        tail.scan_once().await;
        // Started + Message.
        let evt1 = rx.recv().await.unwrap();
        let evt2 = rx.recv().await.unwrap();
        assert!(matches!(evt1, AdapterEvent::SessionStarted { .. }));
        assert!(matches!(evt2, AdapterEvent::Message { .. }));
    }
    #[tokio::test]
    async fn quiesce_emits_hibernated_status() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().to_path_buf();
        let (tx, mut rx) = mpsc::channel(64);
        let mut tail = LogTail::new(
            LogTailConfig {
                sessions_root: sessions.clone(),
                poll_interval: Duration::from_millis(10),
                quiesce: Duration::from_millis(1),
                offsets_path: None,
            },
            tx,
            CancellationToken::new(),
        );
        let path = sessions.join("s1.jsonl");
        std::fs::write(&path, "{\"role\":\"assistant\",\"text\":\"hi\"}\n").unwrap();
        tail.scan_once().await;
        drain(&mut rx);
        tokio::time::sleep(Duration::from_millis(20)).await;
        tail.scan_once().await;
        match rx.recv().await.unwrap() {
            AdapterEvent::Status { tempo, .. } => assert_eq!(tempo.as_deref(), Some("hibernated")),
            other => panic!("expected hibernated status, got {other:?}"),
        }
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
                quiesce: Duration::from_hours(1),
                offsets_path: None,
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
        // regression: a session whose rollout id was discovered by the
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
                quiesce: Duration::from_hours(1),
                offsets_path: None,
            },
            tx,
            CancellationToken::new(),
        );
        // Rollout filename whose stem ends with the inventory-discovered id.
        let id = "019ea66a-cf6e-73b1";
        let path = sessions.join(format!("rollout-2026-{id}.jsonl"));
        std::fs::write(&path, "{\"role\":\"assistant\",\"text\":\"real transcript\"}\n").unwrap();
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
                quiesce: Duration::from_hours(1),
                offsets_path: None,
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
                spawn_relay: false,
            },
        );
        tail.set_owned(registry);
        let path = sessions.join(format!("rollout-{id}.jsonl"));
        std::fs::write(&path, "{\"role\":\"assistant\",\"text\":\"x\"}\n").unwrap();
        tail.scan_once().await;
        assert!(rx.try_recv().is_err(), "owned rollout file must not be tailed");
    }
    #[tokio::test]
    async fn subagent_rollout_nests_under_dispatched_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().to_path_buf();
        let (tx, mut rx) = mpsc::channel(64);
        let mut tail = LogTail::new(
            LogTailConfig {
                sessions_root: sessions.clone(),
                poll_interval: Duration::from_millis(10),
                quiesce: Duration::from_hours(1),
                offsets_path: None,
            },
            tx,
            CancellationToken::new(),
        );
        let exec = "019f832c-6301-7053-8000-0000000000e1";
        crate::dispatch_codex::register_dispatch_thread(exec, "DISPATCH-LT-1");
        let child = "019f832c-6301-7053-8000-0000000000e2";
        let path = sessions.join(format!("rollout-{child}.jsonl"));
        std::fs::write(
            &path,
            format!(
                r#"{{"type":"session_meta","payload":{{"id":"{child}","cwd":"/workspace","source":{{"subAgent":{{"thread_spawn":{{"parent_thread_id":"{exec}"}}}}}}}}}}"#
            ),
        )
        .unwrap();
        tail.scan_once().await;
        let AdapterEvent::SessionStarted { local_id, meta } = rx.recv().await.unwrap() else {
            panic!("expected SessionStarted")
        };
        assert_eq!(local_id, child);
        assert_eq!(meta.parent_local_id.as_deref(), Some("DISPATCH-LT-1"));
        assert_eq!(meta.extra["subagent"], json!(true));
    }
    #[tokio::test]
    async fn snake_case_subagent_rollout_nests_under_its_parent() {
        // codex 0.153 writes `source.subagent` (snake_case) + top-level
        // `parent_thread_id`; the parent is a plain app-server session, so the
        // child must nest under it directly.
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().to_path_buf();
        let (tx, mut rx) = mpsc::channel(64);
        let mut tail = LogTail::new(
            LogTailConfig {
                sessions_root: sessions.clone(),
                poll_interval: Duration::from_millis(10),
                quiesce: Duration::from_hours(1),
                offsets_path: None,
            },
            tx,
            CancellationToken::new(),
        );
        let parent = "01a09a6a-a692-72e1-bdef-be54a77174b2";
        let child = "01a09a80-8a73-7051-82d5-68e3c0b97770";
        let path = sessions.join(format!("rollout-2026-09-13T13-21-50-{child}.jsonl"));
        std::fs::write(
            &path,
            format!(
                r#"{{"timestamp":"2026-09-13T11:21:50.456Z","type":"session_meta","payload":{{"session_id":"{parent}","id":"{child}","forked_from_id":"{parent}","parent_thread_id":"{parent}","cwd":"/workspace","originator":"cctui","source":{{"subagent":{{"thread_spawn":{{"parent_thread_id":"{parent}","depth":1,"agent_path":"/root/noms_fondateur","agent_nickname":"Ohm","agent_role":null}}}}}},"thread_source":"subagent","agent_nickname":"Ohm"}}}}"#
            ),
        )
        .unwrap();
        tail.scan_once().await;
        let AdapterEvent::SessionStarted { local_id, meta } = rx.recv().await.unwrap() else {
            panic!("expected SessionStarted")
        };
        assert_eq!(local_id, child);
        assert_eq!(meta.parent_local_id.as_deref(), Some(parent));
        assert_eq!(meta.extra["subagent"], json!(true));
    }
    #[tokio::test]
    async fn orphan_subagent_rollout_is_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().to_path_buf();
        let (tx, mut rx) = mpsc::channel(64);
        let mut tail = LogTail::new(
            LogTailConfig {
                sessions_root: sessions.clone(),
                poll_interval: Duration::from_millis(10),
                quiesce: Duration::from_hours(1),
                offsets_path: None,
            },
            tx,
            CancellationToken::new(),
        );
        let child = "019f832c-6301-7053-8000-0000000000e3";
        let path = sessions.join(format!("rollout-{child}.jsonl"));
        std::fs::write(
            &path,
            format!(
                r#"{{"type":"session_meta","payload":{{"id":"{child}","source":{{"subAgent":"review"}}}}}}"#
            ),
        )
        .unwrap();
        tail.scan_once().await;
        assert!(rx.try_recv().is_err(), "orphan subagent rollout must be skipped");
    }
    async fn started_meta_for_exec(originator: &str, id: &str) -> SessionMeta {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().to_path_buf();
        let (tx, mut rx) = mpsc::channel(64);
        let mut tail = LogTail::new(
            LogTailConfig {
                sessions_root: sessions.clone(),
                poll_interval: Duration::from_millis(10),
                quiesce: Duration::from_hours(1),
                offsets_path: None,
            },
            tx,
            CancellationToken::new(),
        );
        let path = sessions.join(format!("rollout-{id}.jsonl"));
        std::fs::write(
            &path,
            format!(
                r#"{{"type":"session_meta","payload":{{"id":"{id}","cwd":"/workspace","source":"exec","originator":"{originator}"}}}}"#
            ),
        )
        .unwrap();
        tail.scan_once().await;
        let AdapterEvent::SessionStarted { meta, .. } = rx.recv().await.unwrap() else {
            panic!("expected SessionStarted")
        };
        meta
    }
    #[tokio::test]
    async fn exec_rollout_nests_under_stamped_launcher() {
        let id = "019f832c-6301-7053-8000-0000000000f1";
        let launcher = "356d4dde-659c-47c7-8a3c-aa4e5c44b50a";
        let meta = started_meta_for_exec(&format!("cctui-parent.{launcher}"), id).await;
        assert_eq!(meta.parent_local_id.as_deref(), Some(launcher));
        assert_eq!(meta.extra["subagent"], json!(true));
    }
    #[tokio::test]
    async fn unstamped_exec_rollout_stays_parentless() {
        let id = "019f832c-6301-7053-8000-0000000000f2";
        let meta = started_meta_for_exec("codex_exec", id).await;
        assert_eq!(meta.parent_local_id, None);
        assert_eq!(meta.extra.get("subagent"), None);
    }
    #[tokio::test]
    async fn self_referential_stamp_is_refused() {
        let id = "019f832c-6301-7053-8000-0000000000f3";
        let meta = started_meta_for_exec(&format!("cctui-parent.{id}"), id).await;
        assert_eq!(meta.parent_local_id, None, "a self-parent would cycle the heartbeat CTE");
    }
    #[tokio::test]
    async fn quiesced_rollout_is_not_replayed_on_rediscovery() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let (tx, mut rx) = mpsc::channel(64);
        let mut tail = LogTail::new(
            LogTailConfig {
                sessions_root: sessions.clone(),
                poll_interval: Duration::from_millis(10),
                quiesce: Duration::from_millis(1),
                offsets_path: Some(tmp.path().join("offsets.json")),
            },
            tx,
            CancellationToken::new(),
        );
        let path = sessions.join("s1.jsonl");
        std::fs::write(&path, "{\"role\":\"assistant\",\"text\":\"hi\"}\n").unwrap();
        tail.scan_once().await;
        drain(&mut rx); // Started + Message + mark
        tokio::time::sleep(Duration::from_millis(20)).await;
        tail.scan_once().await;
        assert!(matches!(rx.recv().await.unwrap(), AdapterEvent::Status { .. }));
        tail.scan_once().await;
        tail.scan_once().await;
        assert!(rx.try_recv().is_err(), "quiesced rollout must stay silent");
        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(f, "{{\"role\":\"assistant\",\"text\":\"more\"}}").unwrap();
        tail.scan_once().await;
        match rx.recv().await.unwrap() {
            AdapterEvent::Message { payload, .. } => {
                assert_eq!(payload["text"], json!("more"));
            }
            other => panic!("expected only the appended line, got {other:?}"),
        }
        assert!(texts(&drain(&mut rx)).is_empty());
    }
    #[tokio::test]
    async fn idle_rollout_hibernates_and_resumes_without_replay() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let (tx, mut rx) = mpsc::channel(64);
        let mut tail = LogTail::new(
            LogTailConfig {
                sessions_root: sessions.clone(),
                poll_interval: Duration::from_millis(10),
                quiesce: Duration::from_millis(1),
                offsets_path: Some(tmp.path().join("offsets.json")),
            },
            tx,
            CancellationToken::new(),
        );
        let path = sessions.join("s1.jsonl");
        std::fs::write(&path, "{\"role\":\"assistant\",\"text\":\"hi\"}\n").unwrap();
        tail.scan_once().await;
        assert!(matches!(rx.recv().await.unwrap(), AdapterEvent::SessionStarted { .. }));
        assert!(matches!(rx.recv().await.unwrap(), AdapterEvent::Message { .. }));
        drain(&mut rx);

        tokio::time::sleep(Duration::from_millis(20)).await;
        tail.scan_once().await;
        match rx.recv().await.unwrap() {
            AdapterEvent::Status { tempo, .. } => assert_eq!(tempo.as_deref(), Some("hibernated")),
            other => panic!("an idle rollout must hibernate, got {other:?}"),
        }
        tail.scan_once().await;
        assert!(rx.try_recv().is_err(), "hibernation must be emitted once");

        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(f, "{{\"role\":\"assistant\",\"text\":\"more\"}}").unwrap();
        tail.scan_once().await;
        match rx.recv().await.unwrap() {
            AdapterEvent::Message { payload, .. } => assert_eq!(payload["text"], json!("more")),
            other => panic!("expected only the appended line, got {other:?}"),
        }
        assert!(texts(&drain(&mut rx)).is_empty());

        std::fs::remove_file(&path).unwrap();
        tail.scan_once().await;
        assert!(
            matches!(rx.recv().await.unwrap(), AdapterEvent::SessionEnded { .. }),
            "a removed rollout is the one real end"
        );
    }
    #[tokio::test]
    async fn offsets_survive_restart() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let offsets_path = Some(tmp.path().join("offsets.json"));
        let cfg = LogTailConfig {
            sessions_root: sessions.clone(),
            poll_interval: Duration::from_millis(10),
            quiesce: Duration::from_hours(1),
            offsets_path,
        };
        let path = sessions.join("s1.jsonl");
        std::fs::write(&path, "{\"role\":\"assistant\",\"text\":\"hi\"}\n").unwrap();
        {
            let (tx, mut rx) = mpsc::channel(64);
            let mut tail = LogTail::new(cfg.clone(), tx, CancellationToken::new());
            tail.scan_once().await;
            rx.recv().await.unwrap();
            rx.recv().await.unwrap();
        }
        let (tx, mut rx) = mpsc::channel(64);
        let mut tail = LogTail::new(cfg, tx, CancellationToken::new());
        tail.scan_once().await;
        assert!(rx.try_recv().is_err(), "restart must not replay the rollout");
    }
    #[tokio::test]
    async fn fast_tick_stats_only_recent_date_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().to_path_buf();
        for day in 1..=20 {
            let dir = sessions.join(format!("2020/01/{day:02}"));
            std::fs::create_dir_all(&dir).unwrap();
            for n in 0..100 {
                std::fs::write(dir.join(format!("rollout-{day}-{n}.jsonl")), "").unwrap();
            }
        }
        let today = sessions.join(chrono::Local::now().date_naive().format("%Y/%m/%d").to_string());
        std::fs::create_dir_all(&today).unwrap();
        let live = today.join(format!("{ROLLOUT}.jsonl"));
        write_turns(&live, 0..1);

        let (tx, mut rx) = mpsc::channel(256);
        let mut tail = tail_with(&sessions, None, tx);
        tail.scan_once().await;
        assert_eq!(tail.index.files.len(), 2001);
        assert!(tail.index.stats >= 2001, "first tick is a full walk");
        let _ = drain(&mut rx);

        write_turns(&live, 1..2);
        tail.scan_once().await;
        assert!(tail.index.stats <= 2, "fast tick stat {} files", tail.index.stats);
        assert_eq!(tail.index.files.len(), 2001, "historic rollouts stay known");
        assert_eq!(texts(&drain(&mut rx)), vec!["turn 1"]);
    }
    #[tokio::test]
    async fn fast_tick_keeps_tailing_an_old_dated_rollout() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().to_path_buf();
        let old = sessions.join("2020/01/01");
        std::fs::create_dir_all(&old).unwrap();
        let path = old.join(format!("{ROLLOUT}.jsonl"));
        write_turns(&path, 0..1);
        let (tx, mut rx) = mpsc::channel(64);
        let mut tail = tail_with(&sessions, None, tx);
        tail.scan_once().await;
        let _ = drain(&mut rx);
        write_turns(&path, 1..2);
        tail.scan_once().await;
        assert_eq!(texts(&drain(&mut rx)), vec!["turn 1"]);
    }
    #[tokio::test]
    async fn quiet_rollout_is_opened_once_for_the_mark_check() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let path = sessions.join(format!("{ROLLOUT}.jsonl"));
        let offsets_path = tmp.path().join("offsets.json");
        write_turns(&path, 0..2);
        {
            let (tx, _rx) = mpsc::channel(64);
            let mut tail = tail_with(&sessions, Some(offsets_path.clone()), tx);
            tail.scan_once().await;
        }
        let (tx, mut rx) = mpsc::channel(64);
        let mut tail = tail_with(&sessions, Some(offsets_path), tx);
        tail.set_resume_marks(Arc::new(Mutex::new(HashMap::from([("other".to_owned(), 1)]))));
        tail.scan_once().await;
        assert_eq!(tail.quiet_ids.get(&path).map(String::as_str), Some(ROLLOUT_ID));
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, "x".repeat(10)).unwrap();
        tail.scan_once().await;
        assert_eq!(
            tail.quiet_ids.get(&path).map(String::as_str),
            Some(ROLLOUT_ID),
            "cached id is reused, not re-derived from the rewritten file"
        );
        assert!(drain(&mut rx).is_empty(), "quiet rollout without a mark stays untracked");
    }
}
