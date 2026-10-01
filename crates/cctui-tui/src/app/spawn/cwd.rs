//! The working-directory row: recent dirs, Tab completion and the git badge.
//!
//! The badge rule is `cctui_clientcore::git::git_badge`, shared with the web UI.
//! The debounce matches `cwdGitInfo.ts` so a slow link never freezes the dialog:
//! only the latest lookup may deliver.

use cctui_proto::git::GitInfo;

use super::super::action::Effect;

/// Matches `GIT_INFO_DEBOUNCE_MS` in `cwdGitInfo.ts`.
pub const GIT_DEBOUNCE_MS: i64 = 300;

/// Completions and recent dirs offered under the Dir row.
#[derive(Debug, Clone, Default)]
pub struct CwdState {
    /// Directories the machine offered for the current prefix.
    pub completions: Vec<String>,
    /// Recently used dirs, from `/sessions/recent-dirs`.
    pub recent: Vec<String>,
    /// Open dropdown: which of [`Self::offered`] is highlighted.
    pub picking: Option<usize>,
    pub badge: Option<cctui_clientcore::git::GitBadge>,
    /// Set when the path is readable but not a directory.
    pub not_a_dir: bool,
    /// Clock reading the pending git lookup is due at.
    pub due_ms: Option<i64>,
    /// `(machine, path)` the badge describes, so a stale reply is dropped.
    pub asked: Option<(String, String)>,
}

impl CwdState {
    /// What the dropdown lists: completions when the machine offered any, else
    /// the recent dirs.
    #[must_use]
    pub fn offered(&self) -> &[String] {
        if self.completions.is_empty() { &self.recent } else { &self.completions }
    }

    /// The badge as one short string: `main`, `main ⑂`, `detached @abc1234`.
    #[must_use]
    pub fn badge_text(&self) -> Option<String> {
        if self.not_a_dir {
            return Some("not a directory".to_owned());
        }
        let badge = self.badge.as_ref()?;
        Some(if badge.worktree {
            format!("{} ⑂ worktree", badge.text)
        } else {
            badge.text.clone()
        })
    }

    /// Arms the debounced lookup for a new path.
    pub fn on_edit(&mut self, clock_ms: i64) {
        self.completions.clear();
        self.picking = None;
        self.not_a_dir = false;
        self.due_ms = Some(clock_ms + GIT_DEBOUNCE_MS);
    }

    /// Fires the lookup once it is due. Only the latest `(machine, path)` is
    /// asked for, and only its reply is accepted.
    pub fn on_tick(&mut self, clock_ms: i64, machine: &str, path: &str) -> Vec<Effect> {
        let Some(due) = self.due_ms else { return Vec::new() };
        if clock_ms < due {
            return Vec::new();
        }
        self.due_ms = None;
        if machine.is_empty() || path.is_empty() {
            self.badge = None;
            return Vec::new();
        }
        self.asked = Some((machine.to_owned(), path.to_owned()));
        vec![Effect::FetchGitInfo { machine_id: machine.to_owned(), path: path.to_owned() }]
    }

    /// A reply for a path the operator has already moved on from is dropped.
    pub fn git_loaded(&mut self, machine: &str, path: &str, info: Option<&GitInfo>) {
        if self.asked.as_ref().is_none_or(|(m, p)| m != machine || p != path) {
            return;
        }
        self.badge = cctui_clientcore::git::git_badge(info);
        self.not_a_dir = false;
    }

    pub fn git_failed(&mut self, machine: &str, path: &str) {
        if self.asked.as_ref().is_none_or(|(m, p)| m != machine || p != path) {
            return;
        }
        self.badge = None;
        self.not_a_dir = true;
    }

    /// Steps the dropdown, opening it on the first press.
    pub fn step(&mut self, delta: i32) {
        let len = self.offered().len();
        if len == 0 {
            return;
        }
        let next = match self.picking {
            None if delta < 0 => len - 1,
            None => 0,
            Some(at) if delta < 0 => (at + len - 1) % len,
            Some(at) => (at + 1) % len,
        };
        self.picking = Some(next);
    }

    /// The highlighted entry, for Enter.
    #[must_use]
    pub fn selected(&self) -> Option<&str> {
        self.offered().get(self.picking?).map(String::as_str)
    }

    pub const fn close(&mut self) {
        self.picking = None;
    }
}

/// What `Tab` on the Dir row does: finish the one offer, extend to what several
/// share, or ask the machine when nothing is held yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Complete {
    /// Replace the path with this.
    Replace(String),
    /// Nothing is held: ask the machine for `prefix`.
    Fetch,
    /// Several offers and nothing more is shared.
    Ambiguous,
    None,
}

#[must_use]
pub fn complete(state: &CwdState, path: &str) -> Complete {
    if state.completions.is_empty() {
        return Complete::Fetch;
    }
    let matching: Vec<&String> = state.completions.iter().filter(|c| c.starts_with(path)).collect();
    match matching.len() {
        0 => Complete::None,
        1 => Complete::Replace(matching[0].clone()),
        _ => {
            let shared = shared_prefix(&matching);
            if shared.len() > path.len() { Complete::Replace(shared) } else { Complete::Ambiguous }
        }
    }
}

fn shared_prefix(words: &[&String]) -> String {
    let Some(first) = words.first() else { return String::new() };
    let mut len = first.chars().count();
    for word in &words[1..] {
        len = len.min(first.chars().zip(word.chars()).take_while(|(a, b)| a == b).count());
    }
    first.chars().take(len).collect()
}

#[cfg(test)]
mod tests {
    use cctui_proto::git::GitInfo;

    use super::{Complete, CwdState, GIT_DEBOUNCE_MS, complete};
    use crate::app::action::Effect;

    fn repo(branch: Option<&str>, sha: Option<&str>, worktree: bool) -> GitInfo {
        GitInfo {
            is_repo: true,
            branch: branch.map(str::to_owned),
            detached_sha: sha.map(str::to_owned),
            is_worktree: worktree,
            ..GitInfo::default()
        }
    }

    fn asked(path: &str) -> CwdState {
        CwdState { asked: Some(("m-1".to_owned(), path.to_owned())), ..CwdState::default() }
    }

    #[test]
    fn the_badge_reads_the_branch_the_worktree_and_a_detached_head() {
        let mut s = asked("/w");
        s.git_loaded("m-1", "/w", Some(&repo(Some("main"), None, false)));
        assert_eq!(s.badge_text().as_deref(), Some("main"));

        s.git_loaded("m-1", "/w", Some(&repo(Some("main"), None, true)));
        assert_eq!(s.badge_text().as_deref(), Some("main ⑂ worktree"));

        s.git_loaded("m-1", "/w", Some(&repo(None, Some("abc1234567"), false)));
        assert_eq!(s.badge_text().as_deref(), Some("detached @abc1234"));
    }

    #[test]
    fn a_path_that_is_not_a_repo_or_not_a_dir_says_so() {
        let mut s = asked("/w");
        s.git_loaded("m-1", "/w", Some(&GitInfo::default()));
        assert_eq!(s.badge_text(), None, "not a repo is simply no badge");

        s.git_failed("m-1", "/w");
        assert_eq!(s.badge_text().as_deref(), Some("not a directory"));
    }

    #[test]
    fn a_reply_for_an_abandoned_path_is_dropped() {
        let mut s = asked("/new");
        s.git_loaded("m-1", "/old", Some(&repo(Some("stale"), None, false)));
        assert_eq!(s.badge_text(), None);
        s.git_failed("m-1", "/old");
        assert!(!s.not_a_dir, "a stale failure must not mark the current path");
    }

    #[test]
    fn a_burst_of_keystrokes_costs_one_lookup_once_it_settles() {
        let mut s = CwdState::default();
        s.on_edit(1_000);
        assert!(s.on_tick(1_000 + GIT_DEBOUNCE_MS - 1, "m-1", "/w").is_empty());
        match s.on_tick(1_000 + GIT_DEBOUNCE_MS, "m-1", "/w").as_slice() {
            [Effect::FetchGitInfo { machine_id, path }] => {
                assert_eq!(machine_id, "m-1");
                assert_eq!(path, "/w");
            }
            other => panic!("expected one lookup, got {}", other.len()),
        }
        assert!(s.on_tick(9_999, "m-1", "/w").is_empty(), "and it does not fire twice");
    }

    #[test]
    fn an_empty_machine_or_path_resolves_to_no_badge_without_a_request() {
        let mut s = asked("/w");
        s.badge = Some(cctui_clientcore::git::GitBadge {
            text: "main".to_owned(),
            worktree: false,
            sha: None,
        });
        s.on_edit(0);
        assert!(s.on_tick(GIT_DEBOUNCE_MS, "", "/w").is_empty());
        assert_eq!(s.badge, None);
    }

    #[test]
    fn editing_clears_the_completions_and_closes_the_dropdown() {
        let mut s = CwdState {
            completions: vec!["/a".to_owned()],
            picking: Some(0),
            not_a_dir: true,
            ..CwdState::default()
        };
        s.on_edit(0);
        assert!(s.completions.is_empty());
        assert!(s.picking.is_none());
        assert!(!s.not_a_dir);
    }

    #[test]
    fn the_dropdown_shows_completions_when_there_are_any_and_recents_otherwise() {
        let mut s = CwdState { recent: vec!["/home/dev/cctui".to_owned()], ..CwdState::default() };
        assert_eq!(s.offered(), ["/home/dev/cctui"]);
        s.completions = vec!["/srv/a".to_owned(), "/srv/b".to_owned()];
        assert_eq!(s.offered().len(), 2);
    }

    #[test]
    fn the_dropdown_wraps_in_both_directions_and_enter_reads_the_pick() {
        let mut s =
            CwdState { recent: vec!["/a".to_owned(), "/b".to_owned()], ..CwdState::default() };
        assert_eq!(s.selected(), None, "closed until stepped");
        s.step(1);
        assert_eq!(s.selected(), Some("/a"));
        s.step(1);
        assert_eq!(s.selected(), Some("/b"));
        s.step(1);
        assert_eq!(s.selected(), Some("/a"), "wraps");
        s.step(-1);
        assert_eq!(s.selected(), Some("/b"));
        s.close();
        assert_eq!(s.selected(), None);
    }

    #[test]
    fn stepping_an_empty_dropdown_does_nothing() {
        let mut s = CwdState::default();
        s.step(1);
        assert!(s.picking.is_none());
    }

    #[test]
    fn tab_asks_the_machine_when_it_holds_nothing_yet() {
        assert_eq!(complete(&CwdState::default(), "/ho"), Complete::Fetch);
    }

    #[test]
    fn one_offer_completes_and_several_extend_to_what_they_share() {
        let one =
            CwdState { completions: vec!["/home/dev/cctui".to_owned()], ..CwdState::default() };
        assert_eq!(complete(&one, "/home/dev/cc"), Complete::Replace("/home/dev/cctui".to_owned()));

        let several = CwdState {
            completions: vec!["/home/dev/cctui".to_owned(), "/home/dev/cctui-wt".to_owned()],
            ..CwdState::default()
        };
        assert_eq!(
            complete(&several, "/home/dev/cc"),
            Complete::Replace("/home/dev/cctui".to_owned()),
            "they share more than was typed"
        );
        assert_eq!(
            complete(&several, "/home/dev/cctui"),
            Complete::Ambiguous,
            "and nothing more is shared"
        );
    }

    #[test]
    fn a_prefix_nothing_matches_completes_to_nothing() {
        let s = CwdState { completions: vec!["/srv".to_owned()], ..CwdState::default() };
        assert_eq!(complete(&s, "/home"), Complete::None);
    }
}
