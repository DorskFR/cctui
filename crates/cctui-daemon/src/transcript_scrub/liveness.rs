use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::Value;

use super::{Roots, Stamp};

/// Errs toward "live": any doubt keeps a file from being rewritten.
#[derive(Debug, Default)]
pub struct Liveness {
    sessions: HashSet<String>,
    project_dirs: HashSet<String>,
    /// `None` where open files cannot be listed (no `/proc`): every file may
    /// be open, so none is rewritten.
    open: Option<HashSet<PathBuf>>,
    projects_root: PathBuf,
    now: Option<SystemTime>,
}

impl Liveness {
    #[must_use]
    pub fn probe(roots: &Roots) -> Self {
        let mut live = Self {
            projects_root: roots.claude_projects.clone(),
            open: open_files(),
            now: Some(SystemTime::now()),
            ..Self::default()
        };
        live.read_registry(&roots.claude_registry);
        live
    }

    #[must_use]
    pub fn new(projects_root: PathBuf, now: SystemTime) -> Self {
        Self { projects_root, now: Some(now), open: Some(HashSet::new()), ..Self::default() }
    }

    #[must_use]
    pub fn without_open_files(mut self) -> Self {
        self.open = None;
        self
    }

    #[must_use]
    pub const fn knows_open_files(&self) -> bool {
        self.open.is_some()
    }

    pub fn add_session(&mut self, session: &str, cwd: Option<&str>) {
        self.sessions.insert(session.to_owned());
        if let Some(cwd) = cwd {
            self.project_dirs.insert(normalize(cwd.trim_end_matches('/')));
        }
    }

    pub fn add_open(&mut self, path: PathBuf) {
        if let Some(open) = &mut self.open {
            open.insert(path);
        }
    }

    fn read_registry(&mut self, dir: &Path) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            let Some(value) = std::fs::read(&path)
                .ok()
                .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
            else {
                continue;
            };
            let pid = value
                .get("pid")
                .and_then(Value::as_i64)
                .or_else(|| path.file_stem()?.to_str()?.parse().ok());
            if pid.is_some_and(|pid| !alive(pid)) {
                continue;
            }
            let cwd = value.get("cwd").and_then(Value::as_str);
            match value.get("sessionId").and_then(Value::as_str) {
                Some(session) => self.add_session(session, cwd),
                None => {
                    if let Some(cwd) = cwd {
                        self.project_dirs.insert(normalize(cwd.trim_end_matches('/')));
                    }
                }
            }
        }
    }

    #[must_use]
    pub fn is_live(&self, path: &Path, stamp: &Stamp, idle: Duration) -> bool {
        let recent = match (stamp.modified, self.now) {
            (Some(modified), Some(now)) => {
                !now.duration_since(modified).is_ok_and(|age| age >= idle)
            }
            _ => true,
        };
        recent
            || self.held_open(path)
            || self.names_live_session(path)
            || self.in_live_project(path)
    }

    fn held_open(&self, path: &Path) -> bool {
        let Some(open) = &self.open else { return true };
        open.contains(path) || std::fs::canonicalize(path).is_ok_and(|p| open.contains(&p))
    }

    fn names_live_session(&self, path: &Path) -> bool {
        let stem = path.file_stem().and_then(|s| s.to_str());
        stem.into_iter()
            .chain(path.components().filter_map(|c| c.as_os_str().to_str()))
            .any(|name| self.sessions.contains(name))
    }

    fn in_live_project(&self, path: &Path) -> bool {
        path.strip_prefix(&self.projects_root)
            .ok()
            .and_then(|rel| rel.components().next())
            .and_then(|c| c.as_os_str().to_str())
            .is_some_and(|dir| self.project_dirs.contains(&normalize(dir)))
    }
}

/// Claude Code's project-dir encoding has varied between versions; mapping
/// every non-alphanumeric to `-` on both sides matches all of them.
fn normalize(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

/// A pid this user may not signal is still a live process.
pub(super) fn alive(pid: i64) -> bool {
    let Some(pid) = i32::try_from(pid).ok().and_then(rustix::process::Pid::from_raw) else {
        return false;
    };
    match rustix::process::test_kill_process(pid) {
        Ok(()) => true,
        Err(err) => err != rustix::io::Errno::SRCH,
    }
}

fn open_files() -> Option<HashSet<PathBuf>> {
    let procs = std::fs::read_dir("/proc").ok()?;
    let mut out = HashSet::new();
    for proc in procs.flatten() {
        let is_pid =
            proc.file_name().to_str().is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit()));
        if !is_pid {
            continue;
        }
        let Ok(fds) = std::fs::read_dir(proc.path().join("fd")) else { continue };
        for fd in fds.flatten() {
            if let Ok(target) = std::fs::read_link(fd.path()) {
                out.insert(target);
            }
        }
    }
    Some(out)
}
