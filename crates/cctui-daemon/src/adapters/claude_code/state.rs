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
    /// The post-reset session id. `/clear` (and `/compact`) rotate the live
    /// session into a NEW transcript file; the claude binary records the new
    /// id here while leaving `sessionId` pinned to the immutable spawn id.
    /// This is the only place the rotated id surfaces — the control socket's
    /// `list` op keeps reporting the stale spawn `sessionId`.
    #[serde(default)]
    pub resume_session_id: Option<String>,
    /// The immutable spawn session id (`sessionId`). Together with
    /// `resume_session_id` and `cwd` this is everything a hibernated worker
    /// needs to be revived via a resume `dispatch`.
    #[serde(default)]
    pub session_id: Option<String>,
    /// Working directory the worker ran in — required by the resume
    /// `dispatch` payload.
    #[serde(default)]
    pub cwd: Option<String>,
    /// `createdAt` (ISO-8601): stable session origin; server `registered_at`
    /// resets on re-discovery, so age lies after a daemon restart.
    #[serde(default)]
    pub created_at: Option<String>,
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
            resume_session_id: get_str("resumeSessionId"),
            session_id: get_str("sessionId"),
            cwd: get_str("cwd"),
            created_at: get_str("createdAt"),
            children,
        }
    }

    /// Write `name` (and `nameSource:"user"`) into the on-disk `state.json`
    /// for `short`, preserving every other field. The claude daemon has no
    /// rename op on its control socket, and our status poll reads the display
    /// name from this file — so a rename must land here or it is silently
    /// reverted on the next tick.
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
                "createdAt": "2026-07-15T19:25:30.428Z",
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
        assert_eq!(s.created_at.as_deref(), Some("2026-07-15T19:25:30.428Z"));
        assert_eq!(s.children.len(), 1);
        assert_eq!(s.proto_children()[0].kind, "pr");
        // No reset has happened, so the post-reset id is absent.
        assert_eq!(s.resume_session_id, None);
    }

    #[test]
    fn reads_resume_session_id_after_reset() {
        // After `/clear`, the claude binary leaves `sessionId` pinned to the
        // immutable spawn id and records the rotated id in `resumeSessionId`.
        // The daemon must follow the latter to keep tailing the live transcript.
        let tmp = tempfile::tempdir().unwrap();
        let short = "deadbeef";
        let job_dir = tmp.path().join(short);
        std::fs::create_dir(&job_dir).unwrap();
        std::fs::write(
            job_dir.join("state.json"),
            br#"{
                "sessionId": "0995ecd5-62a9-4333-bbc4-9e62f44578ad",
                "resumeSessionId": "c2337470-90a0-4f85-b3db-4e5b25ffeea7",
                "cwd": "/home/me/repo"
            }"#,
        )
        .unwrap();
        let s = StateJson::read(tmp.path(), short).unwrap();
        assert_eq!(s.resume_session_id.as_deref(), Some("c2337470-90a0-4f85-b3db-4e5b25ffeea7"));
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
