//! `~/.config/cctui/tui-state.json`: what the TUI remembers between runs.
//!
//! Separate from `tui.toml`, which the user writes by hand and the TUI only
//! ever reads — this file is the TUI's own scratchpad and is rewritten freely.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Group keys are `<parent id>/<group key>`, section keys are group labels.
///
/// Both sets hold *overrides*, not absolute state: a subagent group's default is
/// open below [`INLINE_THRESHOLD`] and folded at or above it, so the stored key
/// flips whichever default applies. Sections default to open.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiState {
    pub toggled_groups: BTreeSet<String>,
    pub folded_sections: BTreeSet<String>,
    /// Whether the conversation's todo/subagent sidebar is showing.
    pub sidebar_open: bool,
    /// A probe state that reports everything open, so a caller can enumerate the
    /// groups a fully-unfolded list would show. Never persisted.
    #[serde(skip)]
    pub force_open: bool,
}

/// Subagent groups smaller than this render inline; larger ones start folded.
pub const INLINE_THRESHOLD: usize = 3;

impl UiState {
    /// Whether a subagent group of `total` agents shows its rows.
    #[must_use]
    pub fn group_open(&self, id: &str, total: usize) -> bool {
        self.force_open || (total < INLINE_THRESHOLD) != self.toggled_groups.contains(id)
    }

    /// Returns whether the group ended up open.
    pub fn toggle_group(&mut self, id: &str, total: usize) -> bool {
        if !self.toggled_groups.remove(id) {
            self.toggled_groups.insert(id.to_owned());
        }
        self.group_open(id, total)
    }

    #[must_use]
    pub fn section_open(&self, key: &str) -> bool {
        self.force_open || !self.folded_sections.contains(key)
    }

    /// Reports everything open, whatever is really folded.
    #[must_use]
    pub fn probe() -> Self {
        Self { force_open: true, ..Self::default() }
    }

    pub fn toggle_section(&mut self, key: &str) -> bool {
        if !self.folded_sections.remove(key) {
            self.folded_sections.insert(key.to_owned());
        }
        self.section_open(key)
    }

    /// Fold every group and section that is currently open, or — when nothing is
    /// open — unfold the lot. `groups` is every `(id, total)` on screen.
    pub fn fold_all(&mut self, groups: &[(String, usize)], sections: &[&str]) {
        let anything_open = groups.iter().any(|(id, total)| self.group_open(id, *total))
            || sections.iter().any(|key| self.section_open(key));
        self.toggled_groups.clear();
        self.folded_sections.clear();
        // Both directions override a default, so which groups get a key flips
        // with the direction: folding overrides the small ones, opening the big.
        let overridden = |total: usize| (total < INLINE_THRESHOLD) == anything_open;
        for (id, total) in groups {
            if overridden(*total) {
                self.toggled_groups.insert(id.clone());
            }
        }
        if !anything_open {
            return;
        }
        for key in sections {
            self.folded_sections.insert((*key).to_owned());
        }
    }
}

/// `$CCTUI_TUI_STATE` wins so a test (or a second instance) can point elsewhere.
#[must_use]
pub fn state_path() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("CCTUI_TUI_STATE") {
        return Some(PathBuf::from(explicit));
    }
    Some(dirs::config_dir()?.join("cctui").join("tui-state.json"))
}

/// A missing or unreadable file is simply the defaults: remembered fold state is
/// never worth failing a startup over.
#[must_use]
pub fn load() -> UiState {
    state_path().map(|path| load_from(&path)).unwrap_or_default()
}

#[must_use]
pub fn load_from(path: &Path) -> UiState {
    let Ok(text) = std::fs::read_to_string(path) else { return UiState::default() };
    serde_json::from_str(&text).unwrap_or_else(|err| {
        tracing::warn!(%err, path = %path.display(), "ignoring an unreadable tui-state.json");
        UiState::default()
    })
}

pub fn save(state: &UiState) {
    let Some(path) = state_path() else { return };
    save_to(&path, state);
}

pub fn save_to(path: &Path, state: &UiState) {
    if let Some(dir) = path.parent()
        && let Err(err) = std::fs::create_dir_all(dir)
    {
        tracing::warn!(%err, "cannot create the cctui config dir");
        return;
    }
    match serde_json::to_vec_pretty(state) {
        Ok(bytes) => {
            if let Err(err) = std::fs::write(path, bytes) {
                tracing::warn!(%err, path = %path.display(), "cannot write tui-state.json");
            }
        }
        Err(err) => tracing::warn!(%err, "cannot serialise the TUI state"),
    }
}

#[cfg(test)]
mod tests {
    use super::{INLINE_THRESHOLD, UiState};

    #[test]
    fn a_small_group_starts_open_and_a_big_one_starts_folded() {
        let state = UiState::default();
        assert!(state.group_open("p/plain", INLINE_THRESHOLD - 1));
        assert!(!state.group_open("p/plain", INLINE_THRESHOLD));
        assert!(!state.group_open("p/plain", 12));
    }

    #[test]
    fn toggling_flips_whichever_default_applies() {
        let mut state = UiState::default();
        assert!(!state.toggle_group("p/plain", 2));
        assert!(state.toggle_group("p/plain", 2));
        assert!(state.toggle_group("p/wf:r", 12));
        assert!(!state.toggle_group("p/wf:r", 12));
    }

    #[test]
    fn sections_start_open_and_toggle_both_ways() {
        let mut state = UiState::default();
        assert!(state.section_open("working"));
        assert!(!state.toggle_section("working"));
        assert!(!state.section_open("working"));
        assert!(state.toggle_section("working"));
    }

    #[test]
    fn fold_all_closes_everything_then_opens_everything() {
        let mut state = UiState::default();
        let groups = vec![("p/plain".to_owned(), 2), ("p/wf:r".to_owned(), 12)];
        let sections = ["pinned", "working"];

        state.fold_all(&groups, &sections);
        assert!(!state.group_open("p/plain", 2));
        assert!(!state.group_open("p/wf:r", 12));
        assert!(!state.section_open("working"));

        state.fold_all(&groups, &sections);
        assert!(state.group_open("p/plain", 2));
        assert!(state.group_open("p/wf:r", 12));
        assert!(state.section_open("working"));
    }

    #[test]
    fn fold_all_reopens_a_group_that_was_folded_by_default() {
        let mut state = UiState::default();
        let groups = vec![("p/wf:r".to_owned(), 12)];
        assert!(!state.group_open("p/wf:r", 12));
        state.fold_all(&groups, &["working"]);
        assert!(!state.group_open("p/wf:r", 12));
        state.fold_all(&groups, &["working"]);
        assert!(state.group_open("p/wf:r", 12), "unfolding everything opens the big groups too");
    }

    #[test]
    fn the_probe_reports_everything_open() {
        let mut state = UiState::probe();
        assert!(state.group_open("p/wf:r", 99));
        state.toggle_section("working");
        assert!(state.section_open("working"));
    }

    #[test]
    fn the_state_survives_a_round_trip_through_json() {
        let mut state = UiState::default();
        state.toggle_group("p/wf:r", 12);
        state.toggle_section("done");
        let text = serde_json::to_string(&state).expect("serialises");
        let back: UiState = serde_json::from_str(&text).expect("deserialises");
        assert_eq!(back, state);
        assert!(back.group_open("p/wf:r", 12));
        assert!(!back.section_open("done"));
    }

    #[test]
    fn the_fold_state_survives_a_restart() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("nested").join("tui-state.json");

        let mut state = UiState::default();
        state.toggle_group("s-p/wf:run-1", 12);
        state.toggle_section("done");
        super::save_to(&path, &state);

        let back = super::load_from(&path);
        assert_eq!(back, state);
        assert!(back.group_open("s-p/wf:run-1", 12));
        assert!(!back.section_open("done"));
    }

    #[test]
    fn a_missing_or_corrupt_file_is_the_defaults() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let missing = dir.path().join("absent.json");
        assert_eq!(super::load_from(&missing), UiState::default());

        let broken = dir.path().join("broken.json");
        std::fs::write(&broken, "not json").expect("writes");
        assert_eq!(super::load_from(&broken), UiState::default());
    }

    #[test]
    fn an_empty_document_is_the_defaults() {
        let state: UiState = serde_json::from_str("{}").expect("deserialises");
        assert_eq!(state, UiState::default());
    }
}
