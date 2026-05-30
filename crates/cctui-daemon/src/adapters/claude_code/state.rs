//! Read identity fields from `~/.claude/jobs/<short>/state.json`.
//!
//! The `list` op returns lagging/null `name`/`intent` on long-running
//! sessions (§4.3 of the protocol doc). The on-disk `state.json` is the
//! source of truth for identity. We read it on every poll tick (file is
//! small, kernel page-cache makes this cheap) rather than firing a
//! `notify` watcher per session.

use std::path::{Path, PathBuf};

use cctui_proto::adapter::SessionChild;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct StateJson {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub intent: Option<String>,
    #[serde(default)]
    pub activity: Option<String>,
    /// Model the session runs on, parsed from `respawnFlags` (`--model`).
    #[serde(default)]
    pub model: Option<String>,
    /// Reasoning effort, parsed from `respawnFlags` (`--effort`).
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub children: Vec<StateChild>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StateChild {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub href: String,
    #[serde(default)]
    pub kind: String,
}

impl StateJson {
    pub fn read(jobs_root: &Path, short: &str) -> Option<Self> {
        let path = jobs_root.join(short).join("state.json");
        let bytes = std::fs::read(&path).ok()?;
        // The claude binary writes camelCase (`sessionId`, `nameSource`,
        // etc). serde's #[serde(default)] tolerates missing fields; we
        // explicitly map `sessionId` since rustfmt-naming differs.
        let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
        Some(Self::from_value(&v))
    }

    fn from_value(v: &serde_json::Value) -> Self {
        let get_str = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_owned);
        let children = v
            .get("children")
            .and_then(|x| x.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|c| {
                        Some(StateChild {
                            id: c.get("id").and_then(|x| x.as_str())?.to_owned(),
                            href: c.get("href").and_then(|x| x.as_str())?.to_owned(),
                            kind: c.get("kind").and_then(|x| x.as_str())?.to_owned(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        // Model/effort live in the `respawnFlags` array the claude binary
        // writes (e.g. `["--name","x","--model","opus[1m]","--effort","low"]`).
        let flags: Vec<String> = v
            .get("respawnFlags")
            .and_then(|x| x.as_array())
            .map(|arr| arr.iter().filter_map(|f| f.as_str().map(str::to_owned)).collect())
            .unwrap_or_default();
        let flag_val = |name: &str| {
            flags.iter().position(|f| f == name).and_then(|i| flags.get(i + 1)).cloned()
        };
        Self {
            name: get_str("name"),
            intent: get_str("intent"),
            activity: get_str("activity"),
            model: flag_val("--model"),
            effort: flag_val("--effort"),
            children,
        }
    }

    /// Write `name` (and `nameSource:"user"`) into the on-disk `state.json`
    /// for `short`, preserving every other field. The claude daemon has no
    /// rename op on its control socket, and our status poll reads the display
    /// name from this file — so a rename must land here or it is silently
    /// reverted on the next tick (CCT-133).
    pub fn write_name(jobs_root: &Path, short: &str, name: &str) -> std::io::Result<()> {
        let path = jobs_root.join(short).join("state.json");
        let bytes = std::fs::read(&path)?;
        let mut v: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let Some(obj) = v.as_object_mut() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "state.json is not a JSON object",
            ));
        };
        obj.insert("name".to_owned(), serde_json::Value::String(name.to_owned()));
        obj.insert("nameSource".to_owned(), serde_json::Value::String("user".to_owned()));
        // Write to a sibling temp file then rename for an atomic replace, so a
        // concurrent reader never observes a truncated file.
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(&v)?)?;
        std::fs::rename(&tmp, &path)
    }

    #[must_use]
    pub fn proto_children(&self) -> Vec<SessionChild> {
        self.children
            .iter()
            .map(|c| SessionChild { id: c.clone().id, href: c.clone().href, kind: c.clone().kind })
            .collect()
    }
}

/// Default jobs root: `<HOME>/.claude/jobs`.
#[must_use]
pub fn default_jobs_root() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")).join(".claude").join("jobs")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_minimal_state_json() {
        let tmp = tempfile::tempdir().unwrap();
        let short = "deadbeef";
        let job_dir = tmp.path().join(short);
        std::fs::create_dir(&job_dir).unwrap();
        std::fs::write(
            job_dir.join("state.json"),
            br#"{
                "name": "fix bug",
                "intent": "/work CCT-1",
                "cwd": "/home/me/repo",
                "sessionId": "6e189420-f9a4-493f-b3d9-e0a80ac254c1",
                "children": [
                    {"id": "1972", "href": "https://github.com/o/r/pull/1972", "kind": "pr"}
                ]
            }"#,
        )
        .unwrap();
        let s = StateJson::read(tmp.path(), short).unwrap();
        assert_eq!(s.name.as_deref(), Some("fix bug"));
        assert_eq!(s.intent.as_deref(), Some("/work CCT-1"));
        assert_eq!(s.children.len(), 1);
        assert_eq!(s.proto_children()[0].kind, "pr");
    }

    #[test]
    fn write_name_sets_name_and_preserves_other_fields() {
        let tmp = tempfile::tempdir().unwrap();
        let short = "deadbeef";
        let job_dir = tmp.path().join(short);
        std::fs::create_dir(&job_dir).unwrap();
        std::fs::write(
            job_dir.join("state.json"),
            br#"{"name":"old","intent":"/work CCT-1","sessionId":"abc","cwd":"/repo"}"#,
        )
        .unwrap();
        StateJson::write_name(tmp.path(), short, "new name").unwrap();
        let v: serde_json::Value =
            serde_json::from_slice(&std::fs::read(job_dir.join("state.json")).unwrap()).unwrap();
        assert_eq!(v["name"], "new name");
        assert_eq!(v["nameSource"], "user");
        // untouched fields survive
        assert_eq!(v["intent"], "/work CCT-1");
        assert_eq!(v["sessionId"], "abc");
        assert_eq!(v["cwd"], "/repo");
        // re-read through the typed view
        let s = StateJson::read(tmp.path(), short).unwrap();
        assert_eq!(s.name.as_deref(), Some("new name"));
    }

    #[test]
    fn missing_file_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(StateJson::read(tmp.path(), "nonexistent").is_none());
    }
}
