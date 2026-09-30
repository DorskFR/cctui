//! Per-user drafts and spawn memory, shared by the webui and the TUI.
//!
//! Draft keys are opaque: the client owns the namespace, the server enforces
//! only ownership and size. Spawn-memory keys are composite —
//! `m<US>machine<US>cwd` for a machine spawn, `d<US>dispatcher<US>repo` for a
//! dispatch — and the map is stored inside the user settings blob.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
#[cfg(feature = "ts")]
use ts_rs::TS;

/// Longest accepted draft key.
///
/// Spawn slot keys embed a machine id and a working directory, so this is
/// generous; it bounds the index, it does not shape the namespace.
pub const DRAFT_KEY_MAX: usize = 512;

/// Longest accepted draft body.
///
/// Well above any composer prompt (attachments travel separately, as blobs),
/// and below anything that would make the row a problem.
pub const DRAFT_TEXT_MAX: usize = 256 * 1024;

/// How many spawn-memory entries a user keeps.
///
/// Writes past the cap evict the least recently written entries.
pub const SPAWN_MEMORY_CAP: usize = 50;

/// The composite-key separator, the ASCII unit separator.
///
/// Absent from machine ids, dispatcher ids, working directories and repo
/// names, and — unlike NUL — storable in the JSONB settings blob.
pub const KEY_SEP: char = '\u{1f}';

/// Key prefix of a session's unsent composer text.
pub const COMPOSER_DRAFT_PREFIX: &str = "cctui_draft_";

/// Key prefix of a session's sent-prompt history.
pub const SESSION_HISTORY_PREFIX: &str = "cctui_history_";

/// Key of the global spawn-prompt history.
pub const PROMPT_HISTORY_KEY: &str = "cctui_prompt_history";

/// Draft key of one session's composer.
#[must_use]
pub fn composer_draft_key(session_id: &str) -> String {
    format!("{COMPOSER_DRAFT_PREFIX}{session_id}")
}

/// Draft key of one session's prompt history.
#[must_use]
pub fn session_history_key(session_id: &str) -> String {
    format!("{SESSION_HISTORY_PREFIX}{session_id}")
}

/// The session behind a composer-draft key, or `None` for any other key.
#[must_use]
pub fn draft_session_id(key: &str) -> Option<&str> {
    key.strip_prefix(COMPOSER_DRAFT_PREFIX)
}

/// The session behind a prompt-history key, or `None` for any other key.
#[must_use]
pub fn history_session_id(key: &str) -> Option<&str> {
    key.strip_prefix(SESSION_HISTORY_PREFIX)
}

/// One stored draft.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct Draft {
    pub key: String,
    pub text: String,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// `GET /api/v1/drafts` — every draft the caller owns, newest first. The TUI
/// pulls this once at startup instead of a request per session.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct DraftList {
    pub drafts: Vec<Draft>,
}

/// `PUT /api/v1/drafts/{key}`. An empty `text` deletes the row, so the client
/// does not have to branch between `PUT` and `DELETE` as a field is cleared.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct PutDraftRequest {
    pub text: String,
}

/// The spawn configuration remembered for one target.
///
/// Mirrors the fields the spawn form recalls; `account_provider` is carried for
/// display only, the form recomputes it from the account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct SpawnMemoryEntry {
    #[serde(default)]
    pub adapter_id: String,
    #[serde(default)]
    pub model_claude: String,
    #[serde(default)]
    pub model_codex: String,
    #[serde(default)]
    pub model_account: String,
    #[serde(default)]
    pub effort_claude: String,
    #[serde(default)]
    pub effort_codex: String,
    #[serde(default)]
    pub account: String,
    #[serde(default)]
    pub account_provider: String,
    #[serde(default)]
    pub permission_mode: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
    /// The spawn profile the configuration came from, when it came from one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    /// Last-write time, epoch milliseconds. Drives LRU eviction and the
    /// "latest directory on this machine" recall.
    #[serde(default)]
    pub at: i64,
}

/// `GET`/`PUT /api/v1/spawn-memory` — the whole map, keyed by target.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct SpawnMemoryPayload {
    #[serde(default)]
    pub entries: BTreeMap<String, SpawnMemoryEntry>,
}

/// The memory key of a machine spawn target.
#[must_use]
pub fn machine_memory_key(machine_id: &str, working_dir: &str) -> String {
    format!("m{KEY_SEP}{machine_id}{KEY_SEP}{}", normalize_dir(working_dir))
}

/// The memory key of a dispatch spawn target.
#[must_use]
pub fn dispatch_memory_key(dispatcher_id: &str, repo: &str) -> String {
    format!("d{KEY_SEP}{dispatcher_id}{KEY_SEP}{}", repo.trim())
}

/// Canonicalize a working directory for keying: drop trailing slashes so `/w`
/// and `/w/` collapse, but keep the filesystem root intact.
#[must_use]
pub fn normalize_dir(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let stripped = trimmed.trim_end_matches('/');
    if stripped.is_empty() { "/".to_owned() } else { stripped.to_owned() }
}

/// Evict the least recently written entries beyond `cap`, in place.
pub fn evict_spawn_memory(entries: &mut BTreeMap<String, SpawnMemoryEntry>, cap: usize) {
    if entries.len() <= cap {
        return;
    }
    let mut by_age: Vec<(i64, String)> = entries.iter().map(|(k, e)| (e.at, k.clone())).collect();
    by_age.sort_unstable();
    let doomed = by_age.len() - cap;
    for (_, key) in by_age.into_iter().take(doomed) {
        entries.remove(&key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_dir_collapses_trailing_slashes_and_keeps_the_root() {
        assert_eq!(normalize_dir("/home/x/"), "/home/x");
        assert_eq!(normalize_dir("/home/x"), "/home/x");
        assert_eq!(normalize_dir("///"), "/");
        assert_eq!(normalize_dir("  /w/  "), "/w");
        assert_eq!(normalize_dir("   "), "");
    }

    #[test]
    fn memory_keys_are_unit_separated_and_tagged_by_target_kind() {
        assert_eq!(machine_memory_key("m1", "/w/"), "m\u{1f}m1\u{1f}/w");
        assert_eq!(dispatch_memory_key("d1", " org/repo "), "d\u{1f}d1\u{1f}org/repo");
    }

    #[test]
    fn draft_keys_are_the_ones_the_web_ui_stores_and_round_trip() {
        assert_eq!(composer_draft_key("s-1"), "cctui_draft_s-1");
        assert_eq!(session_history_key("s-1"), "cctui_history_s-1");
        assert_eq!(draft_session_id("cctui_draft_s-1"), Some("s-1"));
        assert_eq!(history_session_id("cctui_history_s-1"), Some("s-1"));
        assert_eq!(draft_session_id("cctui_history_s-1"), None);
        assert_eq!(history_session_id("cctui_draft_s-1"), None);
        for other in ["cctui_drafts_imported", PROMPT_HISTORY_KEY, SPAWN_DRAFT_KEY_SAMPLE] {
            assert_eq!(draft_session_id(other), None, "{other} is not a composer draft");
            assert_eq!(history_session_id(other), None, "{other} is not a prompt history");
        }
    }

    /// A spawn-slot key, whose prefix must not read as a session's.
    const SPAWN_DRAFT_KEY_SAMPLE: &str = "cctui_spawn_draft\u{1f}m1\u{1f}/w";

    #[test]
    fn eviction_drops_the_oldest_entries_only_past_the_cap() {
        let mut entries = BTreeMap::new();
        for (at, key) in [(1_i64, "a"), (2, "b"), (3, "c")] {
            entries.insert(key.to_owned(), SpawnMemoryEntry { at, ..sample() });
        }
        evict_spawn_memory(&mut entries, 3);
        assert_eq!(entries.len(), 3, "at the cap nothing is evicted");

        evict_spawn_memory(&mut entries, 2);
        assert!(!entries.contains_key("a"));
        assert!(entries.contains_key("b") && entries.contains_key("c"));
    }

    fn sample() -> SpawnMemoryEntry {
        SpawnMemoryEntry {
            adapter_id: "claude-code".to_owned(),
            model_claude: String::new(),
            model_codex: String::new(),
            model_account: String::new(),
            effort_claude: String::new(),
            effort_codex: String::new(),
            account: String::new(),
            account_provider: String::new(),
            permission_mode: "ask".to_owned(),
            name: String::new(),
            labels: None,
            profile_id: None,
            at: 0,
        }
    }
}
