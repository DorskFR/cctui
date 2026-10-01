//! `~/.config/cctui/recovery/<identity>.json`: drafts the server had not taken
//! when the TUI exited.
//!
//! The server draft store is the real one; this file exists only for the window
//! where it cannot be reached. Two constraints shape it:
//!
//! - It holds prompt text the user typed — possibly secrets — so it is created
//!   owner-only (0600) and deleted as soon as the server has the text.
//! - A host can be shared (that is what the device-login flow is for), so the
//!   file is named after the identity that wrote it and carries that identity
//!   inside it as well. One user's text must never be restored into another
//!   user's account, and must never be overwritten by them either.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Who a recovery file belongs to: one server, one user on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Owner {
    pub server_url: String,
    pub user_id: String,
}

impl Owner {
    #[must_use]
    pub fn new(server_url: &str, user_id: &str) -> Self {
        Self { server_url: server_url.to_owned(), user_id: user_id.to_owned() }
    }

    /// Stable, non-reversible file name for this identity. The server URL and
    /// the user id are not put in a path component: they end up in shell
    /// history, backups and `ls` output.
    #[must_use]
    pub fn file_name(&self) -> String {
        use sha2::{Digest as _, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(self.server_url.as_bytes());
        hasher.update([0]);
        hasher.update(self.user_id.as_bytes());
        let digest = format!("{:x}", hasher.finalize());
        format!("{}.json", &digest[..32])
    }
}

/// The file one quit-time save removes itself from once the server confirms it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub owner: Owner,
    pub path: PathBuf,
}

/// Draft key -> text, the same keys the server store uses, so a restore is a
/// plain merge.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Recovery {
    /// The identity that wrote it, repeated inside the file: the name alone is a
    /// hash, and a file that was copied between hosts or written by an older
    /// build must not be read as ours.
    pub server_url: String,
    pub user_id: String,
    pub drafts: BTreeMap<String, String>,
}

impl Recovery {
    #[must_use]
    pub fn new(owner: &Owner, drafts: BTreeMap<String, String>) -> Self {
        Self { server_url: owner.server_url.clone(), user_id: owner.user_id.clone(), drafts }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.drafts.is_empty()
    }

    /// Whether this file may be handed to `owner`. An unstamped file (no
    /// identity at all) belongs to nobody and is never restored.
    #[must_use]
    pub fn belongs_to(&self, owner: &Owner) -> bool {
        !self.server_url.is_empty()
            && !self.user_id.is_empty()
            && self.server_url == owner.server_url
            && self.user_id == owner.user_id
    }
}

// Where a test points the directory. Per-thread, not global: cargo runs tests in
// parallel threads, and a shared override would have them reading each other's
// temp directories. An env var would need `unsafe`, which the workspace denies.
#[cfg(test)]
thread_local! {
    static TEST_DIR: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub fn set_dir_for_tests(dir: &Path) {
    TEST_DIR.with_borrow_mut(|slot| *slot = Some(dir.to_path_buf()));
}

#[cfg(test)]
fn test_dir() -> Option<PathBuf> {
    TEST_DIR.with_borrow(Clone::clone)
}

#[cfg(not(test))]
const fn test_dir() -> Option<PathBuf> {
    None
}

/// `$CCTUI_TUI_RECOVERY_DIR` wins so a second instance can point elsewhere.
#[must_use]
pub fn recovery_dir() -> Option<PathBuf> {
    if let Some(dir) = test_dir() {
        return Some(dir);
    }
    if let Some(explicit) = std::env::var_os("CCTUI_TUI_RECOVERY_DIR") {
        return Some(PathBuf::from(explicit));
    }
    Some(dirs::config_dir()?.join("cctui").join("recovery"))
}

#[must_use]
pub fn path_for(owner: &Owner) -> Option<PathBuf> {
    Some(recovery_dir()?.join(owner.file_name()))
}

/// What this identity left behind, or nothing. A file that does not stamp this
/// identity is reported as empty and left on disk for whoever owns it.
#[must_use]
pub fn load(owner: &Owner) -> Recovery {
    path_for(owner).map(|path| load_from(&path, owner)).unwrap_or_default()
}

#[must_use]
pub fn load_from(path: &Path, owner: &Owner) -> Recovery {
    let Ok(text) = std::fs::read_to_string(path) else { return Recovery::default() };
    let held: Recovery = match serde_json::from_str(&text) {
        Ok(held) => held,
        Err(err) => {
            tracing::warn!(%err, path = %path.display(), "ignoring an unreadable recovery file");
            return Recovery::default();
        }
    };
    if !held.belongs_to(owner) {
        tracing::warn!(
            path = %path.display(),
            "a recovery file for another identity is left untouched"
        );
        return Recovery::default();
    }
    held
}

/// Replace this identity's file with `drafts`. Called before the saves go out,
/// so a server that never answers still leaves the text on disk.
#[cfg(test)]
pub fn write(owner: &Owner, drafts: BTreeMap<String, String>) {
    if let Some(path) = path_for(owner) {
        write_to(&path, owner, drafts);
    }
}

pub fn write_to(path: &Path, owner: &Owner, drafts: BTreeMap<String, String>) {
    if drafts.is_empty() {
        clear_at(path);
        return;
    }
    save_to(path, &Recovery::new(owner, drafts));
}

/// One draft reached the server: drop it from the file, and drop the file once
/// nothing is left to recover.
pub fn confirm(path: &Path, owner: &Owner, key: &str) {
    let mut held = load_from(path, owner);
    if held.drafts.remove(key).is_none() {
        return;
    }
    if held.is_empty() {
        clear_at(path);
        return;
    }
    save_to(path, &held);
}

fn save_to(path: &Path, recovery: &Recovery) {
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
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;

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
pub fn clear(owner: &Owner) {
    if let Some(path) = path_for(owner) {
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
    use std::collections::BTreeMap;

    use super::{Owner, Recovery, clear_at, confirm, load_from, write_to};

    fn drafts(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
    }

    fn alice() -> Owner {
        Owner::new("https://cctui.example", "11111111-1111-4111-8111-111111111111")
    }

    fn bob() -> Owner {
        Owner::new("https://cctui.example", "22222222-2222-4222-8222-222222222222")
    }

    #[test]
    fn what_was_written_comes_back_and_a_confirm_empties_the_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("nested").join("a.json");
        assert!(load_from(&path, &alice()).is_empty(), "no file is no drafts");

        write_to(&path, &alice(), drafts(&[("draft:s-a", "half a thought"), ("draft:s-b", "two")]));
        let held = load_from(&path, &alice());
        assert_eq!(held.drafts.get("draft:s-a").map(String::as_str), Some("half a thought"));

        confirm(&path, &alice(), "draft:s-a");
        assert_eq!(load_from(&path, &alice()).drafts.len(), 1, "only the confirmed one goes");
        confirm(&path, &alice(), "draft:s-b");
        assert!(!path.exists(), "nothing left to recover, so no file is left behind");
    }

    #[test]
    fn another_identitys_file_is_never_read_and_never_removed() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("a.json");
        write_to(&path, &alice(), drafts(&[("draft:s-a", "alice's unsent prompt")]));

        assert!(
            load_from(&path, &bob()).is_empty(),
            "bob must not inherit alice's text, let alone upload it"
        );
        assert!(path.exists(), "and must not destroy it either");
        assert_eq!(
            load_from(&path, &alice()).drafts.get("draft:s-a").map(String::as_str),
            Some("alice's unsent prompt"),
            "it is still alice's to recover"
        );

        // A confirm by the wrong identity is equally inert.
        confirm(&path, &bob(), "draft:s-a");
        assert!(!load_from(&path, &alice()).is_empty());
    }

    #[test]
    fn the_same_user_on_another_server_is_another_identity() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("a.json");
        write_to(&path, &alice(), drafts(&[("draft:s-a", "text for one instance")]));
        let elsewhere = Owner::new("https://other.example", &alice().user_id);
        assert!(load_from(&path, &elsewhere).is_empty());
        assert_ne!(alice().file_name(), elsewhere.file_name(), "and they do not share a file");
    }

    #[test]
    fn two_identities_do_not_share_a_file_name() {
        assert_ne!(alice().file_name(), bob().file_name());
        assert_eq!(alice().file_name(), alice().file_name(), "and it is stable");
        assert_eq!(std::path::Path::new(&alice().file_name()).extension(), Some("json".as_ref()));
        assert!(
            !alice().file_name().contains("cctui.example"),
            "the server and user must not be readable off the filesystem"
        );
    }

    #[test]
    fn an_unstamped_file_belongs_to_nobody() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("a.json");
        std::fs::write(&path, br#"{"drafts":{"draft:s-a":"from an older build"}}"#).expect("write");
        assert!(load_from(&path, &alice()).is_empty(), "no identity means no claim to it");
        assert!(path.exists());
    }

    #[test]
    fn writing_nothing_removes_the_file_rather_than_leaving_an_empty_one() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("a.json");
        write_to(&path, &alice(), drafts(&[("draft:s-a", "x")]));
        write_to(&path, &alice(), BTreeMap::new());
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_readable_only_by_its_owner() {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("a.json");
        write_to(&path, &alice(), drafts(&[("draft:s-a", "a secret in a prompt")]));
        let mode = std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "prompt text must not be world-readable");
    }

    #[test]
    fn an_unreadable_file_is_no_drafts_rather_than_a_failure() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("a.json");
        std::fs::write(&path, b"{ not json").expect("write");
        assert_eq!(load_from(&path, &alice()), Recovery::default());
        clear_at(&path);
    }
}
