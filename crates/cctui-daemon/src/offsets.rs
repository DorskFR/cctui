//! Persistent (key → byte offset) store shared by the transcript tailers.
//! Best-effort: load/write failures degrade to a one-time replay, which the
//! server dedupes by content hash — never an error.

use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Default)]
pub struct OffsetStore {
    path: Option<PathBuf>,
    map: HashMap<String, u64>,
}

impl OffsetStore {
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

    /// Returns true if anything was removed.
    pub fn retain(&mut self, keep: impl Fn(&str) -> bool) -> bool {
        let before = self.map.len();
        self.map.retain(|k, _| keep(k));
        self.map.len() != before
    }

    pub fn flush(&self) {
        let Some(path) = &self.path else { return };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_vec_pretty(&self.map) {
            Ok(bytes) => {
                if let Err(err) = std::fs::write(path, bytes) {
                    tracing::warn!(%err, ?path, "failed to persist offsets");
                }
            }
            Err(err) => tracing::warn!(%err, "failed to serialise offsets"),
        }
    }
}
