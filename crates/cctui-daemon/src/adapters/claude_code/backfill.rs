//! Historical-session backfill (CCT-86).
//!
//! On daemon start, enumerates `~/.claude/jobs/<short>/state.json` and
//! pushes any session the cursor file doesn't mark complete. For each
//! one we emit `SessionStarted` + tail the full transcript +
//! `SessionEnded` if `state.json` records a terminal `state`.
//!
//! Idempotency: a persistent cursor at
//! `$XDG_CONFIG_HOME/cctui/backfill.json` records the set of `session_ids`
//! we've already backfilled, so daemon restarts don't replay them. The
//! server's `sessions` upsert is keyed on `(machine_id, adapter_id, id)`
//! and tolerates duplicate `SessionStarted` rows, but `stream_events` is
//! append-only and would duplicate on replay — the cursor is what keeps
//! that clean.

use std::collections::HashSet;
use std::path::PathBuf;

use cctui_proto::adapter::{AdapterEvent, EndReason, SessionMeta};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::mpsc;

use super::transcript::{self, OffsetStore};

#[derive(Debug, Clone)]
pub struct BackfillConfig {
    pub jobs_root: PathBuf,
    pub projects_root: PathBuf,
    /// Path to the cursor file recording already-backfilled session ids.
    /// `None` uses the default `$XDG_CONFIG_HOME/cctui/backfill.json`.
    pub cursor_path: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct JobState {
    #[serde(default, alias = "sessionId")]
    session_id: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    activity: Option<String>,
    #[serde(default, alias = "firstTerminalAt")]
    first_terminal_at: Option<String>,
}

impl JobState {
    fn is_terminal(&self) -> bool {
        self.first_terminal_at.is_some()
            || matches!(self.state.as_deref(), Some("done" | "stopped" | "killed" | "failed"))
    }

    fn end_reason(&self) -> EndReason {
        match self.state.as_deref() {
            Some("killed") => EndReason::Killed,
            Some("failed") => EndReason::Crashed { detail: "agent failed".into() },
            _ => EndReason::Completed,
        }
    }
}

#[derive(Debug, Default)]
pub struct CursorFile {
    path: Option<PathBuf>,
    seen: HashSet<String>,
}

impl CursorFile {
    #[must_use]
    pub fn open(path: Option<PathBuf>) -> Self {
        let seen = path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self { path, seen }
    }

    #[must_use]
    pub fn open_default() -> Self {
        let dir = dirs::config_dir().map(|d| d.join("cctui"));
        let path = dir.as_ref().map(|d| d.join("backfill.json"));
        Self::open(path)
    }

    #[must_use]
    pub fn contains(&self, session_id: &str) -> bool {
        self.seen.contains(session_id)
    }

    pub fn mark(&mut self, session_id: String) {
        self.seen.insert(session_id);
    }

    pub fn flush(&self) {
        let Some(path) = &self.path else { return };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(bytes) = serde_json::to_vec_pretty(&self.seen) {
            let _ = std::fs::write(path, bytes);
        }
    }
}

/// Run a single backfill pass. Emits events into `events` and returns
/// the number of sessions backfilled this pass.
pub async fn run_once(
    cfg: &BackfillConfig,
    events: &mpsc::Sender<AdapterEvent>,
    cursor: &mut CursorFile,
    offsets: &mut OffsetStore,
) -> std::io::Result<usize> {
    let Ok(entries) = std::fs::read_dir(&cfg.jobs_root) else {
        return Ok(0);
    };
    let mut count = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(short) = path.file_name().and_then(|s| s.to_str()).map(str::to_owned) else {
            continue;
        };
        let state_path = path.join("state.json");
        let Ok(bytes) = std::fs::read(&state_path) else { continue };
        let Ok(job) = serde_json::from_slice::<JobState>(&bytes) else { continue };
        let Some(session_id) = job.session_id.clone() else { continue };
        if cursor.contains(&session_id) {
            continue;
        }

        backfill_one(&short, &job, &session_id, cfg, events, offsets).await;
        cursor.mark(session_id);
        count += 1;
    }
    if count > 0 {
        cursor.flush();
    }
    Ok(count)
}

async fn backfill_one(
    short: &str,
    job: &JobState,
    session_id: &str,
    cfg: &BackfillConfig,
    events: &mpsc::Sender<AdapterEvent>,
    offsets: &mut OffsetStore,
) {
    let _ = events
        .send(AdapterEvent::SessionStarted {
            local_id: session_id.to_owned(),
            meta: SessionMeta {
                working_dir: job.cwd.clone(),
                extra: json!({
                    "short": short,
                    "backfilled": true,
                    "state": job.state,
                    "activity": job.activity,
                }),
                ..SessionMeta::default()
            },
        })
        .await;

    // Tail the entire transcript (offset 0 → end).
    if let Some(cwd) = job.cwd.as_deref() {
        let path = transcript::transcript_path(&cfg.projects_root, cwd, session_id);
        let off = offsets.get(session_id);
        if let Ok((evts, new_off)) = transcript::tail_once(&path, session_id, off) {
            if new_off != off {
                offsets.set(session_id.to_owned(), new_off);
            }
            for evt in evts {
                let _ = events.send(evt).await;
            }
        }
    }

    if job.is_terminal() {
        let _ = events
            .send(AdapterEvent::SessionEnded {
                local_id: session_id.to_owned(),
                reason: job.end_reason(),
            })
            .await;
    }
}

#[must_use]
pub fn default_cursor_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("cctui").join("backfill.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_state(jobs_root: &std::path::Path, short: &str, body: &str) {
        let dir = jobs_root.join(short);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("state.json"), body).unwrap();
    }

    #[tokio::test]
    async fn backfill_emits_started_for_each_unseen_session() {
        let tmp = tempfile::tempdir().unwrap();
        let jobs = tmp.path().join("jobs");
        let projects = tmp.path().join("projects");
        std::fs::create_dir_all(&projects).unwrap();
        write_state(&jobs, "abcd1234", r#"{"sessionId":"sess-1","cwd":"/tmp","state":"working"}"#);
        write_state(
            &jobs,
            "deadbeef",
            r#"{"sessionId":"sess-2","cwd":"/tmp","state":"done","firstTerminalAt":"x"}"#,
        );
        let (tx, mut rx) = mpsc::channel(64);
        let cfg = BackfillConfig {
            jobs_root: jobs,
            projects_root: projects,
            cursor_path: Some(tmp.path().join("cursor.json")),
        };
        let mut cursor = CursorFile::open(cfg.cursor_path.clone());
        let mut offsets = OffsetStore::open(Some(tmp.path().join("offsets.json")));
        let n = run_once(&cfg, &tx, &mut cursor, &mut offsets).await.unwrap();
        assert_eq!(n, 2);
        // Drain — sess-1 emits Started, sess-2 emits Started + Ended.
        let mut started = 0;
        let mut ended = 0;
        while let Ok(evt) = rx.try_recv() {
            match evt {
                AdapterEvent::SessionStarted { .. } => started += 1,
                AdapterEvent::SessionEnded { .. } => ended += 1,
                _ => {}
            }
        }
        assert_eq!(started, 2);
        assert_eq!(ended, 1);
    }

    #[tokio::test]
    async fn backfill_is_idempotent_via_cursor() {
        let tmp = tempfile::tempdir().unwrap();
        let jobs = tmp.path().join("jobs");
        let projects = tmp.path().join("projects");
        std::fs::create_dir_all(&projects).unwrap();
        write_state(&jobs, "0001", r#"{"sessionId":"sess-a","cwd":"/tmp","state":"done"}"#);
        let (tx, _rx) = mpsc::channel(64);
        let cfg = BackfillConfig {
            jobs_root: jobs,
            projects_root: projects,
            cursor_path: Some(tmp.path().join("cursor.json")),
        };
        let mut cursor = CursorFile::open(cfg.cursor_path.clone());
        let mut offsets = OffsetStore::open(Some(tmp.path().join("offsets.json")));
        let n1 = run_once(&cfg, &tx, &mut cursor, &mut offsets).await.unwrap();
        assert_eq!(n1, 1);
        // Reload cursor from disk → should still skip.
        let mut cursor2 = CursorFile::open(cfg.cursor_path.clone());
        let n2 = run_once(&cfg, &tx, &mut cursor2, &mut offsets).await.unwrap();
        assert_eq!(n2, 0);
    }
}
