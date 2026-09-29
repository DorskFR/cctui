//! Which other live sessions share a spawning session's working tree.
//!
//! Two agents in one checkout overwrite each other: they edit the same files,
//! switch branches under each other, and reset or commit work they did not do.
//! The spawn-time `<session-context>` notice is advisory — it never blocks or
//! delays a spawn.
//!
//! Harness-neutral: the roster is fed from the one point every adapter's events
//! pass through, so a claude session learns about a codex or opencode session in
//! the same tree.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use cctui_proto::adapter::AdapterEvent;

/// How many neighbours are named before the list collapses to `+N more`.
const LIST_CAP: usize = 3;

/// A live session sharing the queried working tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Neighbour {
    pub local_id: String,
    pub harness: String,
    /// The session's name, else its intent. `None` for an unnamed session.
    pub label: Option<String>,
    pub status: Option<String>,
    pub since: Instant,
}

impl Neighbour {
    /// `"name" (claude-code, working, started 14m ago)`.
    fn render(&self, now: Instant) -> String {
        let mut parts = vec![self.harness.clone()];
        if let Some(status) = &self.status {
            parts.push(status.clone());
        }
        parts.push(format!("started {} ago", age(now.saturating_duration_since(self.since))));
        let label = self.label.as_deref().unwrap_or("unnamed");
        format!("\"{label}\" ({})", parts.join(", "))
    }
}

#[derive(Debug, Clone)]
struct Live {
    /// Canonicalized working dir, resolved once when the session registers.
    cwd: PathBuf,
    harness: String,
    name: Option<String>,
    intent: Option<String>,
    status: Option<String>,
    since: Instant,
}

impl Live {
    fn label(&self) -> Option<String> {
        self.name.clone().or_else(|| self.intent.clone())
    }
}

/// Live sessions on this daemon, keyed by `local_id`.
#[derive(Default)]
pub struct LiveDirs {
    sessions: Mutex<HashMap<String, Live>>,
}

static GLOBAL: OnceLock<Arc<LiveDirs>> = OnceLock::new();

#[must_use]
pub fn global() -> Arc<LiveDirs> {
    GLOBAL.get_or_init(|| Arc::new(LiveDirs::default())).clone()
}

/// Every live session sharing `cwd`'s working tree, `exclude` (the spawning
/// session) aside, oldest first.
#[must_use]
pub fn cwd_neighbours(cwd: &str, exclude: Option<&str>) -> Vec<Neighbour> {
    global().neighbours(cwd, exclude)
}

impl LiveDirs {
    /// Fold one adapter event in. `adapter_id` is the harness label.
    pub fn observe(&self, adapter_id: &str, event: &AdapterEvent) {
        let Ok(mut sessions) = self.sessions.lock() else { return };
        match event {
            AdapterEvent::SessionStarted { local_id, meta } => {
                let Some(dir) =
                    meta.working_dir.as_deref().map(str::trim).filter(|d| !d.is_empty())
                else {
                    return;
                };
                let now = Instant::now();
                let entry = sessions.entry(local_id.clone()).or_insert_with(|| Live {
                    cwd: PathBuf::new(),
                    harness: String::new(),
                    name: None,
                    intent: None,
                    status: None,
                    since: now,
                });
                entry.cwd = canonical(dir);
                adapter_id.clone_into(&mut entry.harness);
            }
            // Only an already-registered session is updated: a status carries no
            // working dir, so there is nothing to match a new entry on.
            AdapterEvent::Status { local_id, tempo, state, name, intent, .. } => {
                if let Some(entry) = sessions.get_mut(local_id) {
                    if let Some(name) = text(name.as_deref()) {
                        entry.name = Some(name);
                    }
                    if let Some(intent) = text(intent.as_deref()) {
                        entry.intent = Some(intent);
                    }
                    if let Some(status) = text(state.as_deref()).or_else(|| text(tempo.as_deref()))
                    {
                        entry.status = Some(status);
                    }
                }
                // A hibernated or dead worker is no longer holding the tree.
                if matches!(tempo.as_deref(), Some("hibernated" | "dead")) {
                    sessions.remove(local_id);
                }
            }
            AdapterEvent::SessionEnded { local_id, .. } => {
                sessions.remove(local_id);
            }
            _ => {}
        }
    }

    #[must_use]
    pub fn neighbours(&self, cwd: &str, exclude: Option<&str>) -> Vec<Neighbour> {
        let Ok(sessions) = self.sessions.lock() else { return Vec::new() };
        let target = canonical(cwd);
        let mut out: Vec<Neighbour> = sessions
            .iter()
            .filter(|(local_id, _)| Some(local_id.as_str()) != exclude)
            .filter(|(_, live)| shares_tree(&target, &live.cwd))
            .map(|(local_id, live)| Neighbour {
                local_id: local_id.clone(),
                harness: live.harness.clone(),
                label: live.label(),
                status: live.status.clone(),
                since: live.since,
            })
            .collect();
        out.sort_by(|a, b| a.since.cmp(&b.since).then_with(|| a.local_id.cmp(&b.local_id)));
        out
    }

    #[cfg(test)]
    fn note(&self, local_id: &str, harness: &str, cwd: &Path, since: Instant) {
        self.sessions.lock().unwrap().insert(
            local_id.to_owned(),
            Live {
                cwd: canonical(&cwd.to_string_lossy()),
                harness: harness.to_owned(),
                name: None,
                intent: None,
                status: None,
                since,
            },
        );
    }
}

fn text(value: Option<&str>) -> Option<String> {
    value.map(str::trim).filter(|v| !v.is_empty()).map(str::to_owned)
}

/// Resolve symlinks and `..`; a path that does not exist stands as written so a
/// pending or removed directory still compares by string.
fn canonical(path: &str) -> PathBuf {
    let raw = Path::new(path.trim_end_matches('/'));
    std::fs::canonicalize(raw).unwrap_or_else(|_| raw.to_path_buf())
}

/// The root of the working tree containing `path`: the nearest ancestor holding
/// a `.git` entry. A linked worktree's `.git` is a FILE inside the worktree, so
/// its root is the worktree itself and never the main checkout's — which is the
/// isolation agents are told to use. `None` when `path` is not in a repo.
fn work_tree_root(path: &Path) -> Option<PathBuf> {
    path.ancestors().find(|dir| dir.join(".git").exists()).map(Path::to_path_buf)
}

/// Whether two canonicalized dirs are the same working tree: the same repo
/// toplevel, or — outside any repo — the same directory.
fn shares_tree(a: &Path, b: &Path) -> bool {
    match (work_tree_root(a), work_tree_root(b)) {
        (Some(x), Some(y)) => x == y,
        _ => a == b,
    }
}

/// `3s` / `14m` / `1h02`, the compact spelling the notice reads with.
fn age(elapsed: std::time::Duration) -> String {
    let secs = elapsed.as_secs();
    if secs >= 3600 {
        format!("{}h{:02}", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

/// The `shared cwd:` lines for `neighbours`, or `None` when the session is
/// alone — there is no "0 other sessions" line.
#[must_use]
pub fn notice(neighbours: &[Neighbour], now: Instant) -> Option<String> {
    if neighbours.is_empty() {
        return None;
    }
    let n = neighbours.len();
    let plural = if n == 1 { "session" } else { "sessions" };
    let mut line = format!("shared cwd: {n} other live {plural} in this directory: ");
    let listed =
        neighbours.iter().take(LIST_CAP).map(|nb| nb.render(now)).collect::<Vec<_>>().join(", ");
    line.push_str(&listed);
    if n > LIST_CAP {
        let _ = write!(line, ", +{} more", n - LIST_CAP);
    }
    line.push_str(".\n");
    let whose = if n == 1 { "Its" } else { "Their" };
    let _ = writeln!(
        line,
        "  {whose} uncommitted changes are not yours: do not revert, stash, reset, or commit \
         files you did not touch; do not switch branches. Use a git worktree if you need \
         isolation."
    );
    Some(line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cctui_proto::adapter::{EndReason, SessionMeta};

    /// Run `git` in `dir` with signing and the user's identity pinned off, so a
    /// fixture repo never reaches the developer's signing key.
    fn git(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .args(["-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"])
            .args(["-c", "user.email=t@example.com", "-c", "user.name=t"])
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git runs");
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    fn git_repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        git(tmp.path(), &["init", "--quiet"]);
        tmp
    }

    /// A linked worktree of `repo`, checked out at a new branch.
    fn worktree_of(repo: &Path, name: &str) -> PathBuf {
        git(repo, &["commit", "--allow-empty", "-m", "root"]);
        let path = repo.parent().unwrap().join(name);
        git(repo, &["worktree", "add", "-b", name, &path.to_string_lossy()]);
        path
    }

    fn started(local_id: &str, cwd: &Path) -> AdapterEvent {
        AdapterEvent::SessionStarted {
            local_id: local_id.to_owned(),
            meta: SessionMeta {
                working_dir: Some(cwd.to_string_lossy().into_owned()),
                ..SessionMeta::default()
            },
        }
    }

    fn status(local_id: &str, tempo: &str, state: &str, name: Option<&str>) -> AdapterEvent {
        AdapterEvent::Status {
            local_id: local_id.to_owned(),
            tempo: Some(tempo.to_owned()),
            state: Some(state.to_owned()),
            detail: None,
            activity: None,
            name: name.map(str::to_owned),
            intent: None,
            model: None,
            effort: None,
            permission_mode: None,
            children: Vec::new(),
        }
    }

    fn ids(neighbours: &[Neighbour]) -> Vec<String> {
        neighbours.iter().map(|n| n.local_id.clone()).collect()
    }

    #[test]
    fn the_same_path_a_trailing_slash_and_a_subdirectory_all_match() {
        let repo = git_repo();
        let sub = repo.path().join("crates/daemon");
        std::fs::create_dir_all(&sub).unwrap();
        let dirs = LiveDirs::default();
        dirs.observe("claude-code", &started("other", repo.path()));

        let cwd = repo.path().to_string_lossy().into_owned();
        assert_eq!(ids(&dirs.neighbours(&cwd, None)), ["other"]);
        assert_eq!(ids(&dirs.neighbours(&format!("{cwd}/"), None)), ["other"], "trailing slash");
        assert_eq!(
            ids(&dirs.neighbours(&sub.to_string_lossy(), None)),
            ["other"],
            "a subdirectory of the same working tree counts"
        );
    }

    #[test]
    fn a_symlinked_path_to_the_same_tree_matches() {
        let repo = git_repo();
        let link_parent = tempfile::tempdir().unwrap();
        let link = link_parent.path().join("alias");
        std::os::unix::fs::symlink(repo.path(), &link).unwrap();
        let dirs = LiveDirs::default();
        dirs.observe("claude-code", &started("other", &link));
        assert_eq!(ids(&dirs.neighbours(&repo.path().to_string_lossy(), None)), ["other"]);
    }

    #[test]
    fn a_linked_worktree_of_the_same_repo_is_not_a_neighbour() {
        let repo = git_repo();
        let wt = worktree_of(repo.path(), "lane-f");
        let dirs = LiveDirs::default();
        dirs.observe("claude-code", &started("main-checkout", repo.path()));
        assert!(
            dirs.neighbours(&wt.to_string_lossy(), None).is_empty(),
            "a worktree is the isolation agents are told to use"
        );
        assert!(dirs.neighbours(&repo.path().to_string_lossy(), Some("main-checkout")).is_empty());
    }

    #[test]
    fn an_unrelated_directory_is_never_a_neighbour() {
        let repo = git_repo();
        let other = git_repo();
        let dirs = LiveDirs::default();
        dirs.observe("claude-code", &started("other", other.path()));
        assert!(dirs.neighbours(&repo.path().to_string_lossy(), None).is_empty());
        // Two plain, non-repo directories compare by path.
        let plain = tempfile::tempdir().unwrap();
        let dirs = LiveDirs::default();
        dirs.observe("codex", &started("plain", plain.path()));
        assert_eq!(ids(&dirs.neighbours(&plain.path().to_string_lossy(), None)), ["plain"]);
        assert!(dirs.neighbours(&other.path().to_string_lossy(), None).is_empty());
    }

    #[test]
    fn an_ended_or_hibernated_session_stops_being_a_neighbour() {
        let repo = git_repo();
        let cwd = repo.path().to_string_lossy().into_owned();
        let dirs = LiveDirs::default();
        dirs.observe("claude-code", &started("gone", repo.path()));
        dirs.observe("claude-code", &started("asleep", repo.path()));
        assert_eq!(dirs.neighbours(&cwd, None).len(), 2);

        dirs.observe(
            "claude-code",
            &AdapterEvent::SessionEnded { local_id: "gone".into(), reason: EndReason::Completed },
        );
        dirs.observe("claude-code", &status("asleep", "hibernated", "done", None));
        assert!(dirs.neighbours(&cwd, None).is_empty(), "neither is holding the tree any more");
    }

    #[test]
    fn the_spawning_session_never_lists_itself() {
        let repo = git_repo();
        let dirs = LiveDirs::default();
        dirs.observe("claude-code", &started("me", repo.path()));
        assert!(dirs.neighbours(&repo.path().to_string_lossy(), Some("me")).is_empty());
    }

    #[test]
    fn a_status_fills_in_the_name_and_state_of_a_known_session() {
        let repo = git_repo();
        let dirs = LiveDirs::default();
        dirs.observe("claude-code", &started("s1", repo.path()));
        dirs.observe("claude-code", &status("s1", "active", "working", Some("wave-3 integrator")));
        // A status for a session that never reported a working dir registers nothing.
        dirs.observe("codex", &status("unknown", "active", "working", Some("x")));
        let found = dirs.neighbours(&repo.path().to_string_lossy(), None);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].label.as_deref(), Some("wave-3 integrator"));
        assert_eq!(found[0].status.as_deref(), Some("working"));
        assert_eq!(found[0].harness, "claude-code");
    }

    #[test]
    fn sessions_of_every_harness_are_reported_oldest_first() {
        let repo = git_repo();
        let dirs = LiveDirs::default();
        let base = Instant::now();
        dirs.note("newest", "opencode", repo.path(), base);
        dirs.note(
            "oldest",
            "codex",
            repo.path(),
            base.checked_sub(std::time::Duration::from_secs(600)).unwrap(),
        );
        dirs.note(
            "middle",
            "claude-code",
            repo.path(),
            base.checked_sub(std::time::Duration::from_secs(60)).unwrap(),
        );
        assert_eq!(
            ids(&dirs.neighbours(&repo.path().to_string_lossy(), None)),
            ["oldest", "middle", "newest"]
        );
    }

    fn neighbour(local_id: &str, label: Option<&str>, secs: u64, now: Instant) -> Neighbour {
        Neighbour {
            local_id: local_id.to_owned(),
            harness: "claude-code".to_owned(),
            label: label.map(str::to_owned),
            status: Some("working".to_owned()),
            since: now.checked_sub(std::time::Duration::from_secs(secs)).unwrap(),
        }
    }

    #[test]
    fn no_neighbours_means_no_notice_at_all() {
        assert!(notice(&[], Instant::now()).is_none());
    }

    #[test]
    fn one_neighbour_is_named_with_its_harness_status_age_and_the_guidance() {
        let now = Instant::now();
        let text = notice(&[neighbour("s1", Some("wave-3 integrator"), 14 * 60, now)], now)
            .expect("a notice");
        assert!(text.starts_with("shared cwd: 1 other live session in this directory: "), "{text}");
        assert!(text.contains("\"wave-3 integrator\" (claude-code, working, started 14m ago)."));
        assert!(text.contains("Its uncommitted changes are not yours"), "{text}");
        assert!(text.contains("do not switch branches"), "{text}");
        assert!(text.contains("Use a git worktree"), "{text}");
    }

    #[test]
    fn four_neighbours_list_three_and_say_how_many_more() {
        let now = Instant::now();
        let all: Vec<Neighbour> = (1..=4)
            .map(|i| neighbour(&format!("s{i}"), Some(&format!("agent {i}")), 30, now))
            .collect();
        let text = notice(&all, now).expect("a notice");
        assert!(
            text.starts_with("shared cwd: 4 other live sessions in this directory: "),
            "{text}"
        );
        for name in ["agent 1", "agent 2", "agent 3"] {
            assert!(text.contains(name), "{text}");
        }
        assert!(!text.contains("agent 4"), "{text}");
        assert!(text.contains("+1 more."), "{text}");
        assert!(text.contains("Their uncommitted changes are not yours"), "{text}");
    }

    #[test]
    fn an_unnamed_neighbour_still_renders() {
        let now = Instant::now();
        let text = notice(&[neighbour("s1", None, 3, now)], now).unwrap();
        assert!(text.contains("\"unnamed\" (claude-code, working, started 3s ago)"), "{text}");
    }

    #[test]
    fn ages_read_in_seconds_minutes_and_hours() {
        assert_eq!(age(std::time::Duration::from_secs(3)), "3s");
        assert_eq!(age(std::time::Duration::from_secs(14 * 60)), "14m");
        assert_eq!(age(std::time::Duration::from_secs(3600 + 120)), "1h02");
    }
}
