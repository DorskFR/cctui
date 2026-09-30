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
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cctui_proto::adapter::AdapterEvent;
use serde_json::Value;

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
    /// Wall-clock start the adapter reported; `None` leaves the age out rather
    /// than dating the session from when the daemon observed it.
    pub started_at: Option<SystemTime>,
}

impl Neighbour {
    /// `"name" (claude-code, working, started 14m ago)`.
    fn render(&self, now: SystemTime) -> String {
        let mut parts = vec![self.harness.clone()];
        if let Some(status) = &self.status {
            parts.push(status.clone());
        }
        if let Some(elapsed) = self.started_at.and_then(|at| now.duration_since(at).ok()) {
            parts.push(format!("started {} ago", age(elapsed)));
        }
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
    started_at: Option<SystemTime>,
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
                // A replay is not evidence of death: it must not register an
                // unknown session, nor drop one already known to be running.
                if !is_running_session(&meta.extra) {
                    return;
                }
                let started_at = reported_start(&meta.extra);
                let entry = sessions.entry(local_id.clone()).or_insert_with(|| Live {
                    cwd: PathBuf::new(),
                    harness: String::new(),
                    name: None,
                    intent: None,
                    status: None,
                    started_at: None,
                });
                entry.cwd = canonical(dir);
                adapter_id.clone_into(&mut entry.harness);
                if started_at.is_some() {
                    entry.started_at = started_at;
                }
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

    /// Drop archived sessions. Only the server knows they are archived.
    pub fn forget(&self, local_ids: &[String]) {
        let Ok(mut sessions) = self.sessions.lock() else { return };
        for local_id in local_ids {
            sessions.remove(local_id);
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
                started_at: live.started_at,
            })
            .collect();
        out.sort_by_key(|n| (n.started_at.is_none(), n.started_at, n.local_id.clone()));
        out
    }

    #[cfg(test)]
    fn note(&self, local_id: &str, harness: &str, cwd: &Path, started_at: Option<SystemTime>) {
        self.sessions.lock().unwrap().insert(
            local_id.to_owned(),
            Live {
                cwd: canonical(&cwd.to_string_lossy()),
                harness: harness.to_owned(),
                name: None,
                intent: None,
                status: None,
                started_at,
            },
        );
    }
}

/// Whether a `SessionStarted` announces a session running under this daemon or
/// merely a record of one. The codex `thread/list` inventory re-announces every
/// thread on the machine, backfill replays historical transcripts, and a
/// restored registry record has no live driver behind it.
fn is_running_session(extra: &Value) -> bool {
    if extra.get("backfilled").and_then(Value::as_bool) == Some(true)
        || extra.get("replayed").and_then(Value::as_bool) == Some(true)
    {
        return false;
    }
    !extra
        .get("source")
        .and_then(Value::as_str)
        .is_some_and(|source| source.starts_with("codex-thread-list"))
}

/// The `started_at_ms` an adapter stamps into `SessionMeta::extra` as it starts
/// a session, so a later re-announcement cannot re-date it to the present.
#[must_use]
pub fn now_ms() -> u64 {
    u64::try_from(SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis()))
        .unwrap_or(0)
}

/// The session's own start time, as the adapter that owns it reported it:
/// `started_at_ms` (epoch ms) or claude's RFC 3339 `created_at`.
fn reported_start(extra: &Value) -> Option<SystemTime> {
    if let Some(ms) = extra.get("started_at_ms").and_then(Value::as_u64) {
        return Some(UNIX_EPOCH + Duration::from_millis(ms));
    }
    let text = extra.get("created_at").and_then(Value::as_str)?;
    let parsed = chrono::DateTime::parse_from_rfc3339(text).ok()?;
    let ms = u64::try_from(parsed.timestamp_millis()).ok()?;
    Some(UNIX_EPOCH + Duration::from_millis(ms))
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
fn age(elapsed: Duration) -> String {
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
pub fn notice(neighbours: &[Neighbour], now: SystemTime) -> Option<String> {
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
    use serde_json::json;

    /// Run `git` in `dir` with signing and the user's identity pinned off, so a
    /// fixture repo never reaches the developer's signing key.
    ///
    /// Every inherited `GIT_*` variable is dropped: under a git hook `GIT_DIR`
    /// points at the real repository and would take precedence over `dir`.
    fn git(dir: &Path, args: &[&str]) {
        let mut cmd = std::process::Command::new("git");
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("GIT_") {
                cmd.env_remove(key);
            }
        }
        let out = cmd
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

    /// A linked worktree of `repo` on a fresh branch, inside a tempdir the
    /// caller owns — `git worktree add` needs the leaf not to exist yet, so it
    /// is a subpath of that dir. The returned guard must outlive the assertions.
    fn worktree_of(repo: &Path) -> (tempfile::TempDir, PathBuf) {
        git(repo, &["commit", "--allow-empty", "-m", "root"]);
        let branch = format!("wt-{}", uuid::Uuid::new_v4());
        let parent = tempfile::tempdir().unwrap();
        let path = parent.path().join(&branch);
        git(repo, &["worktree", "add", "-b", &branch, &path.to_string_lossy()]);
        (parent, path)
    }

    fn started(local_id: &str, cwd: &Path) -> AdapterEvent {
        started_with(local_id, cwd, Value::Null)
    }

    fn started_with(local_id: &str, cwd: &Path, extra: Value) -> AdapterEvent {
        AdapterEvent::SessionStarted {
            local_id: local_id.to_owned(),
            meta: SessionMeta {
                working_dir: Some(cwd.to_string_lossy().into_owned()),
                extra,
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
        let (_wt_parent, wt) = worktree_of(repo.path());
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
    fn sessions_of_every_harness_are_reported_oldest_first_with_unknown_starts_last() {
        let repo = git_repo();
        let dirs = LiveDirs::default();
        let base = SystemTime::now();
        dirs.note("newest", "opencode", repo.path(), Some(base));
        dirs.note("oldest", "codex", repo.path(), Some(base - Duration::from_mins(10)));
        dirs.note("middle", "claude-code", repo.path(), Some(base - Duration::from_mins(1)));
        dirs.note("undated", "codex", repo.path(), None);
        assert_eq!(
            ids(&dirs.neighbours(&repo.path().to_string_lossy(), None)),
            ["oldest", "middle", "newest", "undated"]
        );
    }

    #[test]
    fn an_inventory_or_backfill_announcement_is_not_a_live_neighbour() {
        let repo = git_repo();
        let cwd = repo.path().to_string_lossy().into_owned();
        let dirs = LiveDirs::default();
        dirs.observe(
            "codex",
            &started_with(
                "inventory",
                repo.path(),
                json!({ "source": "codex-thread-list:user", "observed_at": 1 }),
            ),
        );
        dirs.observe(
            "claude-code",
            &started_with("history", repo.path(), json!({ "backfilled": true })),
        );
        dirs.observe(
            "codex",
            &started_with(
                "restored",
                repo.path(),
                json!({ "source": "codex-app-server", "replayed": true }),
            ),
        );
        assert!(
            dirs.neighbours(&cwd, None).is_empty(),
            "a record of a session is not a session running here"
        );

        dirs.observe(
            "codex",
            &started_with(
                "restored",
                repo.path(),
                json!({ "source": "codex-app-server", "started_at_ms": now_ms() }),
            ),
        );
        assert_eq!(ids(&dirs.neighbours(&cwd, None)), ["restored"]);
        dirs.observe(
            "codex",
            &started_with(
                "restored",
                repo.path(),
                json!({ "source": "codex-app-server", "replayed": true }),
            ),
        );
        assert_eq!(
            ids(&dirs.neighbours(&cwd, None)),
            ["restored"],
            "a reconnect replay of a live session must not evict it"
        );
    }

    #[test]
    fn a_session_the_server_reports_archived_is_dropped() {
        let repo = git_repo();
        let cwd = repo.path().to_string_lossy().into_owned();
        let dirs = LiveDirs::default();
        dirs.observe("claude-code", &started("archived-later", repo.path()));
        dirs.observe("claude-code", &started("kept", repo.path()));
        dirs.forget(&["archived-later".to_owned(), "never-known".to_owned()]);
        assert_eq!(ids(&dirs.neighbours(&cwd, None)), ["kept"]);
    }

    #[test]
    fn an_idle_live_session_is_still_a_neighbour() {
        let repo = git_repo();
        let dirs = LiveDirs::default();
        dirs.observe("codex", &started("waiting", repo.path()));
        dirs.observe("codex", &status("waiting", "active", "idle", Some("awaiting input")));
        let found = dirs.neighbours(&repo.path().to_string_lossy(), None);
        assert_eq!(ids(&found), ["waiting"], "idle work still sits in the tree");
        assert_eq!(found[0].status.as_deref(), Some("idle"));
    }

    #[test]
    fn the_age_comes_from_the_reported_start_not_the_moment_it_was_observed() {
        let repo = git_repo();
        let cwd = repo.path().to_string_lossy().into_owned();
        let dirs = LiveDirs::default();
        let hour_ago = now_ms() - 3_600_000;
        dirs.observe(
            "codex",
            &started_with("stamped", repo.path(), json!({ "started_at_ms": hour_ago })),
        );
        dirs.observe(
            "claude-code",
            &started_with(
                "rfc3339",
                repo.path(),
                json!({ "created_at": "2026-07-15T19:25:30.428Z" }),
            ),
        );
        dirs.observe("opencode", &started("undated", repo.path()));

        let found = dirs.neighbours(&cwd, None);
        let by_id =
            |id: &str| found.iter().find(|n| n.local_id == id).expect("registered").started_at;
        assert_eq!(by_id("stamped"), Some(UNIX_EPOCH + Duration::from_millis(hour_ago)));
        assert_eq!(
            by_id("rfc3339"),
            Some(UNIX_EPOCH + Duration::from_millis(1_784_143_530_428)),
            "claude's createdAt dates the worker, not this observation"
        );
        assert_eq!(by_id("undated"), None);

        let now = SystemTime::now();
        let text = notice(&found, now).expect("a notice");
        assert!(text.contains("started 1h00 ago"), "{text}");
        assert!(text.contains("\"unnamed\" (opencode)"), "no age beats a wrong age: {text}");
    }

    fn neighbour(local_id: &str, label: Option<&str>, secs: u64, now: SystemTime) -> Neighbour {
        Neighbour {
            local_id: local_id.to_owned(),
            harness: "claude-code".to_owned(),
            label: label.map(str::to_owned),
            status: Some("working".to_owned()),
            started_at: Some(now - Duration::from_secs(secs)),
        }
    }

    #[test]
    fn no_neighbours_means_no_notice_at_all() {
        assert!(notice(&[], SystemTime::now()).is_none());
    }

    #[test]
    fn one_neighbour_is_named_with_its_harness_status_age_and_the_guidance() {
        let now = SystemTime::now();
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
        let now = SystemTime::now();
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
        let now = SystemTime::now();
        let text = notice(&[neighbour("s1", None, 3, now)], now).unwrap();
        assert!(text.contains("\"unnamed\" (claude-code, working, started 3s ago)"), "{text}");
    }

    #[test]
    fn ages_read_in_seconds_minutes_and_hours() {
        assert_eq!(age(std::time::Duration::from_secs(3)), "3s");
        assert_eq!(age(std::time::Duration::from_mins(14)), "14m");
        assert_eq!(age(std::time::Duration::from_mins(62)), "1h02");
    }
}
