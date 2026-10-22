//! Durable record of the children `CctuiAgent` calls follow: the pending
//! follows a re-exec'd daemon resumes, and each child's last finished turn for
//! a reattach or an undelivered completion.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Entries older than this are dropped: a follow this old is abandoned, and a
/// result this old is no longer worth handing back.
pub const RETENTION_SECS: i64 = 24 * 3600;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub parent: String,
    /// Unix seconds.
    pub started_at: i64,
    #[serde(default)]
    pub finished: Option<Finished>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finished {
    /// Unix seconds.
    pub at: i64,
    /// The reply frame the follow produced (`ok` + `result` / `error`).
    pub frame: Value,
    /// Whether a relay or the parent's own turn queue received it.
    pub delivered: bool,
}

impl Entry {
    const fn touched_at(&self) -> i64 {
        match &self.finished {
            Some(f) => f.at,
            None => self.started_at,
        }
    }
}

#[derive(Default)]
struct Ledger {
    entries: HashMap<String, Entry>,
    store: Option<PathBuf>,
    loaded: bool,
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Unit tests never touch the developer's real state file.
fn default_store() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    dirs::config_dir().map(|d| d.join("cctui").join("agent-follows.json"))
}

fn ledger() -> MutexGuard<'static, Ledger> {
    static LEDGER: std::sync::LazyLock<Mutex<Ledger>> = std::sync::LazyLock::new(|| {
        Mutex::new(Ledger { store: default_store(), ..Ledger::default() })
    });
    let mut guard = LEDGER.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if !guard.loaded {
        guard.loaded = true;
        if let Some(path) = guard.store.clone() {
            guard.entries = load_from(&path);
        }
    }
    guard
}

fn load_from(path: &Path) -> HashMap<String, Entry> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn persist_to(path: &Path, entries: &HashMap<String, Entry>) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string(entries).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(tmp, path)
}

fn prune(entries: &mut HashMap<String, Entry>, now: i64) {
    entries.retain(|_, e| now - e.touched_at() < RETENTION_SECS);
}

/// Mutate under the lock, then write the file outside it. Best-effort: a
/// failed write never fails the call it records.
fn edit<R>(f: impl FnOnce(&mut HashMap<String, Entry>) -> R) -> R {
    let mut guard = ledger();
    let out = f(&mut guard.entries);
    prune(&mut guard.entries, now());
    let snapshot = guard.store.clone().map(|path| (path, guard.entries.clone()));
    drop(guard);
    if let Some((path, entries)) = snapshot
        && let Err(err) = persist_to(&path, &entries)
    {
        tracing::warn!(%err, path = %path.display(), "CctuiAgent follow ledger persist failed");
    }
    out
}

/// A call started following `child` on behalf of `parent`.
pub fn begin(child: &str, parent: &str) {
    edit(|entries| {
        entries.insert(
            child.to_owned(),
            Entry { parent: parent.to_owned(), started_at: now(), finished: None },
        );
    });
}

/// The follow of `child` produced `frame`.
pub fn finish(child: &str, parent: &str, frame: &Value, delivered: bool) {
    edit(|entries| {
        let entry = entries.entry(child.to_owned()).or_insert_with(|| Entry {
            parent: parent.to_owned(),
            started_at: now(),
            finished: None,
        });
        entry.finished = Some(Finished { at: now(), frame: frame.clone(), delivered });
    });
}

pub fn mark_delivered(child: &str) {
    edit(|entries| {
        if let Some(f) = entries.get_mut(child).and_then(|e| e.finished.as_mut()) {
            f.delivered = true;
        }
    });
}

#[must_use]
pub fn get(child: &str) -> Option<Entry> {
    ledger().entries.get(child).cloned()
}

/// Every live entry, for a fresh daemon image to resume.
#[must_use]
pub fn all() -> Vec<(String, Entry)> {
    let mut guard = ledger();
    prune(&mut guard.entries, now());
    guard.entries.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_finished_follow_survives_a_new_process_image() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent-follows.json");
        let mut entries = HashMap::new();
        entries.insert(
            "child-1".to_owned(),
            Entry {
                parent: "parent-1".into(),
                started_at: now(),
                finished: Some(Finished {
                    at: now(),
                    frame: json!({ "ok": true, "result": "done" }),
                    delivered: false,
                }),
            },
        );
        entries.insert(
            "child-2".to_owned(),
            Entry { parent: "parent-1".into(), started_at: now(), finished: None },
        );
        persist_to(&path, &entries).unwrap();
        assert_eq!(load_from(&path), entries);
    }

    #[test]
    fn stale_entries_are_pruned_by_their_last_activity() {
        let t = now();
        let mut entries = HashMap::new();
        entries.insert(
            "old".to_owned(),
            Entry { parent: "p".into(), started_at: t - RETENTION_SECS - 1, finished: None },
        );
        entries.insert(
            "finished-recently".to_owned(),
            Entry {
                parent: "p".into(),
                started_at: t - RETENTION_SECS - 1,
                finished: Some(Finished { at: t, frame: json!({}), delivered: true }),
            },
        );
        prune(&mut entries, t);
        assert!(!entries.contains_key("old"));
        assert!(entries.contains_key("finished-recently"));
    }

    #[test]
    fn a_missing_or_corrupt_file_reads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_from(&dir.path().join("absent.json")).is_empty());
        let bad = dir.path().join("bad.json");
        std::fs::write(&bad, "{not json").unwrap();
        assert!(load_from(&bad).is_empty());
    }
}
