//! ACP sessions to re-attach after a daemon re-exec, keyed by the agent's
//! `sessionId` (which is also the cctui local id). The gateway credential is
//! never stored: a restore re-pulls it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use cctui_proto::adapter::PermissionMode;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub adapter: String,
    pub key: String,
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
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

    #[must_use]
    pub fn get(&self, local_id: &str) -> Option<Record> {
        self.lock().get(local_id).cloned()
    }

    #[must_use]
    pub fn for_adapter(&self, adapter: &str) -> Vec<(String, Record)> {
        self.lock()
            .iter()
            .filter(|(_, r)| r.adapter == adapter)
            .map(|(id, r)| (id.clone(), r.clone()))
            .collect()
    }

    pub fn upsert(&self, local_id: &str, record: Record) {
        self.mutate(|records| {
            if records.get(local_id) == Some(&record) {
                return false;
            }
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
        let Err(err) = save_to(path, &records) else { return };
        drop(records);
        tracing::warn!(%err, path = %path.display(), "acp: session registry persist failed");
    }
}

#[must_use]
pub fn store_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("cctui").join("acp-sessions.json"))
}

/// The daemon-wide store, shared by every agent row. Unit tests get a
/// pathless one so they never touch the developer's real state file.
#[must_use]
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
    #[cfg(unix)]
    if let Some(dir) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::File::open(dir)?.sync_all()?;
    }
    Ok(())
}

/// A missing file is an empty registry. A corrupt one is set aside, not
/// silently overwritten by the next save, so its sessions can still be found.
#[must_use]
pub fn load_from(path: &Path) -> Records {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Records::new(),
        Err(err) => {
            tracing::warn!(%err, path = %path.display(), "acp: session registry unreadable");
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
                "acp: session registry corrupt, starting empty"
            );
            let _ = std::fs::rename(path, &aside);
            Records::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(adapter: &str) -> Record {
        Record {
            adapter: adapter.to_owned(),
            key: "spawn-key".to_owned(),
            cwd: "/repo".to_owned(),
            model: Some("gemini-2.5-pro".to_owned()),
            effort: Some("high".to_owned()),
            permission_mode: Some(PermissionMode::Auto),
            parent_local_id: Some("parent".to_owned()),
            started_at_ms: 1_784_143_530_428,
        }
    }

    #[test]
    fn a_saved_registry_reloads_identically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("acp-sessions.json");
        let store = SessionStore::at(path.clone());
        store.upsert("s1", record("gemini"));
        store.upsert("s2", record("qwen"));
        let back = SessionStore::at(path.clone()).snapshot();
        assert_eq!(back.len(), 2);
        assert_eq!(back["s1"], record("gemini"));
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(leftovers, vec!["acp-sessions.json".to_owned()], "no temp file left");
    }

    #[test]
    fn each_row_restores_only_its_own_sessions() {
        let store = SessionStore::default();
        store.upsert("s1", record("gemini"));
        store.upsert("s2", record("qwen"));
        let mine = store.for_adapter("gemini");
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].0, "s1");
    }

    #[test]
    fn an_ended_session_is_removed_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("acp-sessions.json");
        let store = SessionStore::at(path.clone());
        store.upsert("s1", record("gemini"));
        store.upsert("s2", record("gemini"));
        store.remove("s1");
        let back = load_from(&path);
        assert!(!back.contains_key("s1"));
        assert!(back.contains_key("s2"));
    }

    #[test]
    fn a_record_without_optional_fields_restores() {
        let back: Records = serde_json::from_str(
            r#"{"s":{"adapter":"gemini","key":"","cwd":"/w","started_at_ms":7}}"#,
        )
        .unwrap();
        assert_eq!(back["s"].permission_mode, None);
        assert_eq!(back["s"].started_at_ms, 7);
    }

    #[test]
    fn a_corrupt_registry_is_set_aside_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("acp-sessions.json");
        std::fs::write(&path, "not json").unwrap();
        assert!(load_from(&path).is_empty());
        assert!(path.with_extension("json.corrupt").exists());
    }
}
