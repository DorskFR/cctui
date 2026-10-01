//! `~/.config/cctui/recovery.json`: drafts the server refused at quit.
//!
//! The server draft store is the real one; this file exists only for the window
//! where it cannot be reached. It holds prompt text the user typed — possibly
//! secrets — so it is written owner-only and deleted as soon as it has been
//! handed back.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Draft key -> text, the same keys the server store uses, so a restore is a
/// plain merge.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Recovery {
    pub drafts: BTreeMap<String, String>,
}

impl Recovery {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.drafts.is_empty()
    }
}

/// Where a test points the file. Setting an env var would need `unsafe`, which
/// the workspace denies.
#[cfg(test)]
static TEST_PATH: std::sync::RwLock<Option<PathBuf>> = std::sync::RwLock::new(None);

#[cfg(test)]
pub fn set_path_for_tests(path: &Path) {
    *TEST_PATH.write().unwrap_or_else(std::sync::PoisonError::into_inner) =
        Some(path.to_path_buf());
}

#[cfg(test)]
fn test_path() -> Option<PathBuf> {
    TEST_PATH.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
}

#[cfg(not(test))]
const fn test_path() -> Option<PathBuf> {
    None
}

/// `$CCTUI_TUI_RECOVERY` wins so a second instance can point elsewhere.
#[must_use]
pub fn recovery_path() -> Option<PathBuf> {
    if let Some(path) = test_path() {
        return Some(path);
    }
    if let Some(explicit) = std::env::var_os("CCTUI_TUI_RECOVERY") {
        return Some(PathBuf::from(explicit));
    }
    Some(dirs::config_dir()?.join("cctui").join("recovery.json"))
}

#[must_use]
pub fn load() -> Recovery {
    recovery_path().map(|path| load_from(&path)).unwrap_or_default()
}

#[must_use]
pub fn load_from(path: &Path) -> Recovery {
    let Ok(text) = std::fs::read_to_string(path) else { return Recovery::default() };
    serde_json::from_str(&text).unwrap_or_else(|err| {
        tracing::warn!(%err, path = %path.display(), "ignoring an unreadable recovery.json");
        Recovery::default()
    })
}

/// Add one draft to the file, keeping whatever is already in it: several saves
/// can fail during the same quit, and each lands separately.
pub fn record(key: &str, text: &str) {
    let Some(path) = recovery_path() else { return };
    record_in(&path, key, text);
}

pub fn record_in(path: &Path, key: &str, text: &str) {
    let mut held = load_from(path);
    held.drafts.insert(key.to_owned(), text.to_owned());
    save_to(path, &held);
}

pub fn save_to(path: &Path, recovery: &Recovery) {
    let Ok(text) = serde_json::to_string_pretty(recovery) else { return };
    if let Some(parent) = path.parent()
        && let Err(err) = std::fs::create_dir_all(parent)
    {
        tracing::warn!(%err, path = %path.display(), "cannot create the recovery directory");
        return;
    }
    if let Err(err) = write_private(path, &text) {
        tracing::warn!(%err, path = %path.display(), "cannot write the recovery file");
    }
}

/// Owner-only from the moment it exists: created with 0600 rather than written
/// and then chmod'ed, so the text is never briefly world-readable.
#[cfg(unix)]
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(text.as_bytes())
}

#[cfg(not(unix))]
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    std::fs::write(path, text)
}

/// Called once the drafts are back in hand: the file must not outlive the
/// restore, or it would overwrite newer text on the next start.
pub fn clear() {
    if let Some(path) = recovery_path() {
        clear_at(&path);
    }
}

pub fn clear_at(path: &Path) {
    if let Err(err) = std::fs::remove_file(path)
        && err.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(%err, path = %path.display(), "cannot remove the recovery file");
    }
}

#[cfg(test)]
mod tests {
    use super::{Recovery, clear_at, load_from, record_in};

    #[test]
    fn a_recorded_draft_comes_back_and_the_file_can_be_cleared() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("nested").join("recovery.json");
        assert!(load_from(&path).is_empty(), "no file is no drafts");

        record_in(&path, "draft:s-a", "half a thought");
        record_in(&path, "draft:s-b", "another");
        let held = load_from(&path);
        assert_eq!(held.drafts.get("draft:s-a").map(String::as_str), Some("half a thought"));
        assert_eq!(held.drafts.len(), 2, "a second failure joins the first");

        clear_at(&path);
        assert!(load_from(&path).is_empty());
        clear_at(&path);
    }

    #[test]
    fn a_later_save_of_the_same_key_wins() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("recovery.json");
        record_in(&path, "draft:s-a", "first");
        record_in(&path, "draft:s-a", "second");
        assert_eq!(load_from(&path).drafts.get("draft:s-a").map(String::as_str), Some("second"));
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_readable_only_by_its_owner() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("recovery.json");
        record_in(&path, "draft:s-a", "a secret in a prompt");
        let mode = std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "prompt text must not be world-readable");
    }

    #[test]
    fn an_unreadable_file_is_no_drafts_rather_than_a_failure() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("recovery.json");
        std::fs::write(&path, b"{ not json").expect("write");
        assert_eq!(load_from(&path), Recovery::default());
    }
}
