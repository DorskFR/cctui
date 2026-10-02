//! Sessions to re-attach after a daemon re-exec. The gateway credential is
//! never stored: a restore re-pulls it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use cctui_proto::adapter::PermissionMode;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub key: String,
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<PermissionMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_local_id: Option<String>,
    pub started_at_ms: u64,
}

pub type Records = BTreeMap<String, Record>;

/// Every mutation snapshots and writes under the same lock, so two sessions
/// ending together can never land an older snapshot over a newer one.
#[derive(Debug, Default)]
pub struct SessionStore {
    path: Option<PathBuf>,
    records: Mutex<Records>,
}

impl SessionStore {
    #[must_use]
    pub fn at(path: PathBuf) -> Self {
        let records = Mutex::new(load_from(&path));
        Self { path: Some(path), records }
    }

    #[must_use]
    pub fn snapshot(&self) -> Records {
        self.lock().clone()
    }

    pub fn upsert(&self, local_id: &str, record: Record) {
        self.mutate(|records| {
            records.insert(local_id.to_owned(), record);
            true
        });
    }

    pub fn remove(&self, local_id: &str) {
        self.mutate(|records| records.remove(local_id).is_some());
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Records> {
        self.records.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn mutate(&self, change: impl FnOnce(&mut Records) -> bool) {
        let mut records = self.lock();
        if !change(&mut records) {
            return;
        }
        let Some(path) = &self.path else { return };
        if let Err(err) = save_to(path, &records) {
            tracing::warn!(%err, path = %path.display(), "opencode: session registry persist failed");
        }
    }
}

#[must_use]
pub fn store_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("cctui").join("opencode-sessions.json"))
}

/// The daemon-wide store. Unit tests get a pathless one so they never touch
/// the developer's real state file.
pub fn global() -> Arc<SessionStore> {
    static STORE: LazyLock<Arc<SessionStore>> = LazyLock::new(|| {
        Arc::new(match store_path() {
            Some(path) if !cfg!(test) => SessionStore::at(path),
            _ => SessionStore::default(),
        })
    });
    Arc::clone(&STORE)
}

pub fn save_to(path: &Path, records: &Records) -> std::io::Result<()> {
    let json = serde_json::to_vec_pretty(records).map_err(std::io::Error::other)?;
    cctui_proto::util::write_private(path, &json)?;
    sync_dir(path)
}

/// The rename is only durable once the directory entry is.
fn sync_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        if let Some(dir) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::File::open(dir)?.sync_all()?;
        }
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// A missing file is an empty registry. An unreadable one is set aside, not
/// silently overwritten by the next save, so its sessions can still be found.
#[must_use]
pub fn load_from(path: &Path) -> Records {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Records::new(),
        Err(err) => {
            tracing::warn!(%err, path = %path.display(), "opencode: session registry unreadable");
            return Records::new();
        }
    };
    match serde_json::from_str(&text) {
        Ok(records) => records,
        Err(err) => {
            let aside = path.with_extension("json.corrupt");
            tracing::warn!(
                %err,
                path = %path.display(),
                aside = %aside.display(),
                "opencode: session registry corrupt, starting empty"
            );
            let _ = std::fs::rename(path, &aside);
            Records::new()
        }
    }
}

/// Group records by launch key: each key gets its own `opencode serve`, and a
/// fork lives on its parent's server.
#[must_use]
pub fn by_key(records: Records) -> BTreeMap<String, Vec<(String, Record)>> {
    let mut groups: BTreeMap<String, Vec<(String, Record)>> = BTreeMap::new();
    for (local_id, record) in records {
        groups.entry(record.key.clone()).or_default().push((local_id, record));
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(key: &str, parent: Option<&str>) -> Record {
        Record {
            key: key.to_owned(),
            cwd: "/repo".to_owned(),
            agent: Some("cctui-builder".to_owned()),
            model: Some("fireworks-ai/kimi".to_owned()),
            permission_mode: Some(PermissionMode::Yolo),
            parent_local_id: parent.map(str::to_owned),
            started_at_ms: 1_784_143_530_428,
        }
    }

    #[test]
    fn a_saved_registry_reloads_identically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("opencode-sessions.json");
        let store = SessionStore::at(path.clone());
        store.upsert("ses_a", record("k1", None));
        store.upsert("ses_b", record("k1", Some("ses_a")));

        let back = SessionStore::at(path.clone()).snapshot();
        assert_eq!(back.len(), 2);
        assert_eq!(back["ses_a"], record("k1", None));
        assert_eq!(back["ses_b"].parent_local_id.as_deref(), Some("ses_a"));
        assert_eq!(back["ses_a"].permission_mode, Some(PermissionMode::Yolo));
        assert_eq!(back["ses_a"].started_at_ms, 1_784_143_530_428);
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(leftovers, vec!["opencode-sessions.json".to_owned()], "no temp file left");
    }

    #[test]
    fn an_ended_session_is_removed_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode-sessions.json");
        let store = SessionStore::at(path.clone());
        store.upsert("ses_a", record("k1", None));
        store.upsert("ses_b", record("k2", None));
        store.remove("ses_a");

        let back = load_from(&path);
        assert!(!back.contains_key("ses_a"), "a stale record would be resurrected on restart");
        assert!(back.contains_key("ses_b"));
    }

    #[test]
    fn a_corrupt_file_starts_empty_and_is_set_aside() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode-sessions.json");
        std::fs::write(&path, "{not json").unwrap();

        let store = SessionStore::at(path.clone());
        assert!(store.snapshot().is_empty());
        assert_eq!(
            std::fs::read_to_string(path.with_extension("json.corrupt")).unwrap(),
            "{not json",
            "the unreadable registry must survive for inspection"
        );
        store.upsert("ses_a", record("k1", None));
        assert!(load_from(&path).contains_key("ses_a"));
    }

    #[test]
    fn a_missing_file_is_an_empty_registry() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_from(&dir.path().join("missing.json")).is_empty());
    }

    #[test]
    fn a_pathless_store_keeps_records_in_memory_only() {
        let store = SessionStore::default();
        store.upsert("ses_a", record("k1", None));
        assert_eq!(store.snapshot().len(), 1);
    }

    #[test]
    fn records_are_grouped_by_launch_key() {
        let mut records = Records::new();
        records.insert("ses_a".to_owned(), record("k1", None));
        records.insert("ses_b".to_owned(), record("k1", Some("ses_a")));
        records.insert("ses_c".to_owned(), record("k2", None));
        let groups = by_key(records);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups["k1"].len(), 2);
        assert_eq!(groups["k2"][0].0, "ses_c");
    }
}
