//! Durable snapshot of the codex [`SessionRegistry`], written on mutation and
//! merged back on adapter startup so a daemon restart / self-update re-exec
//! does not strand hibernated threads. The launch env is never persisted —
//! it carries the gateway credential; a resume with an empty env re-pulls it
//! from the server.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::app_server::{AppServerConfig, SessionRecord, SessionRegistry};

#[derive(Debug, Serialize, Deserialize)]
struct PersistedRecord {
    cwd: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    cfg: AppServerConfig,
}

#[must_use]
pub fn store_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("cctui").join("codex-sessions.json"))
}

#[must_use]
pub fn to_json(records: &HashMap<String, SessionRecord>) -> String {
    let map: std::collections::BTreeMap<&String, PersistedRecord> = records
        .iter()
        .map(|(id, r)| {
            (id, PersistedRecord { cwd: r.cwd.clone(), name: r.name.clone(), cfg: r.cfg.clone() })
        })
        .collect();
    serde_json::to_string_pretty(&map).unwrap_or_else(|_| "{}".to_owned())
}

/// Parse a persisted snapshot. Records come back with an empty env so the
/// first resume re-pulls the gateway credential from the server.
#[must_use]
pub fn from_json(text: &str) -> HashMap<String, SessionRecord> {
    serde_json::from_str::<HashMap<String, PersistedRecord>>(text)
        .map(|map| {
            map.into_iter()
                .map(|(id, r)| {
                    (
                        id,
                        SessionRecord {
                            cfg: r.cfg,
                            cwd: r.cwd,
                            name: r.name,
                            env: std::collections::BTreeMap::new(),
                        },
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn save_to(path: &Path, records: &HashMap<String, SessionRecord>) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, to_json(records))?;
    std::fs::rename(&tmp, path)
}

#[must_use]
pub fn load_from(path: &Path) -> HashMap<String, SessionRecord> {
    std::fs::read_to_string(path).map(|text| from_json(&text)).unwrap_or_default()
}

/// Snapshot the registry to the state file. Best-effort: failures are logged,
/// never fatal to the session that triggered the write.
pub async fn save(registry: &SessionRegistry) {
    // Unit tests exercise registry-mutating helpers; never let them touch the
    // developer's real state file.
    if cfg!(test) {
        return;
    }
    let Some(path) = store_path() else { return };
    let snapshot = registry.lock().await.clone();
    if let Err(err) = save_to(&path, &snapshot) {
        tracing::warn!(%err, path = %path.display(), "codex: session registry persist failed");
    }
}

/// Merge the persisted snapshot into the registry without clobbering records
/// live sessions already inserted. Returns how many records were restored.
pub async fn load(registry: &SessionRegistry) -> usize {
    if cfg!(test) {
        return 0;
    }
    let Some(path) = store_path() else { return 0 };
    merge(registry, load_from(&path)).await
}

pub async fn merge(registry: &SessionRegistry, records: HashMap<String, SessionRecord>) -> usize {
    let mut restored = 0_usize;
    let mut guard = registry.lock().await;
    for (id, record) in records {
        guard.entry(id).or_insert_with(|| {
            restored += 1;
            record
        });
    }
    restored
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(cwd: &str, name: Option<&str>) -> SessionRecord {
        SessionRecord {
            cfg: AppServerConfig {
                model: Some("gpt-5-codex".to_owned()),
                reasoning_effort: Some("high".to_owned()),
                ..AppServerConfig::default()
            },
            cwd: cwd.to_owned(),
            name: name.map(str::to_owned),
            env: std::iter::once(("OPENAI_API_KEY".to_owned(), "secret".to_owned())).collect(),
        }
    }

    #[test]
    fn round_trip_preserves_cfg_and_strips_env() {
        let mut map = HashMap::new();
        map.insert("tid-1".to_owned(), record("/repo", Some("nm")));
        let json = to_json(&map);
        assert!(!json.contains("secret"), "env must never be persisted");
        let back = from_json(&json);
        let r = back.get("tid-1").expect("record survives");
        assert_eq!(r.cwd, "/repo");
        assert_eq!(r.name.as_deref(), Some("nm"));
        assert_eq!(r.cfg.model.as_deref(), Some("gpt-5-codex"));
        assert_eq!(r.cfg.reasoning_effort.as_deref(), Some("high"));
        assert!(r.env.is_empty());
    }

    #[test]
    fn from_json_tolerates_garbage() {
        assert!(from_json("not json").is_empty());
        assert!(from_json("{}").is_empty());
    }

    #[test]
    fn file_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("codex-sessions.json");
        let mut map = HashMap::new();
        map.insert("tid".to_owned(), record("/w", None));
        save_to(&path, &map).unwrap();
        let back = load_from(&path);
        assert_eq!(back.get("tid").map(|r| r.cwd.as_str()), Some("/w"));
        assert!(load_from(&tmp.path().join("missing.json")).is_empty());
    }

    #[tokio::test]
    async fn merge_does_not_clobber_live_records() {
        let registry = SessionRegistry::default();
        registry.lock().await.insert("live".to_owned(), record("/live", Some("keep-me")));
        let mut persisted = HashMap::new();
        persisted.insert("live".to_owned(), record("/stale", Some("stale")));
        persisted.insert("restored".to_owned(), record("/repo", None));
        let restored = merge(&registry, persisted).await;
        assert_eq!(restored, 1);
        let (live_name, restored_cwd) = {
            let guard = registry.lock().await;
            (
                guard.get("live").and_then(|r| r.name.clone()),
                guard.get("restored").map(|r| r.cwd.clone()),
            )
        };
        assert_eq!(live_name.as_deref(), Some("keep-me"));
        assert_eq!(restored_cwd.as_deref(), Some("/repo"));
    }
}
