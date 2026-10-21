//! Claude Code's task list: one `<id>.json` per live task under
//! `<config>/tasks/<session-id>/`.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use cctui_proto::adapter::AdapterEvent;
use cctui_proto::api::TodoEntry;
use serde::Deserialize;

use super::Driver;

const MAX_TASK_FILE_BYTES: u64 = 256 * 1024;

type Fingerprint = Vec<(String, Option<SystemTime>, u64)>;

#[derive(Debug)]
pub(super) struct TaskWatch {
    dir: PathBuf,
    seen: Option<Fingerprint>,
    sent: Option<Fingerprint>,
    /// Only a folder that once had tasks may send an empty list.
    ///
    /// One that never did would wipe a `TodoWrite` list.
    had_tasks: bool,
}

impl TaskWatch {
    pub(super) const fn new(dir: PathBuf) -> Self {
        Self { dir, seen: None, sent: None, had_tasks: false }
    }

    pub(super) fn retarget(&mut self, dir: &Path) {
        if self.dir != dir {
            *self = Self { had_tasks: self.had_tasks, ..Self::new(dir.to_path_buf()) };
        }
    }

    /// The list once a change has held still for a whole poll.
    ///
    /// An unreadable folder sends nothing, leaving the server folding the tool
    /// events.
    pub(super) fn poll(&mut self) -> Option<Vec<TodoEntry>> {
        let fp = fingerprint(&self.dir)?;
        if self.seen.as_ref() != Some(&fp) {
            self.seen = Some(fp);
            return None;
        }
        if self.sent.as_ref() == Some(&fp) {
            return None;
        }
        self.sent = Some(fp);
        let list = read_task_list(&self.dir);
        if list.is_empty() && !self.had_tasks {
            return None;
        }
        self.had_tasks = !list.is_empty();
        Some(list)
    }
}

fn fingerprint(dir: &Path) -> Option<Fingerprint> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Some(Vec::new()),
        Err(err) => {
            tracing::debug!(%err, dir = %dir.display(), "task list unreadable");
            return None;
        }
    };
    let mut fp: Fingerprint = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().into_string().ok().filter(|n| is_task_file(n))?;
            let meta = e.metadata().ok()?;
            Some((name, meta.modified().ok(), meta.len()))
        })
        .collect();
    fp.sort();
    Some(fp)
}

fn is_task_file(name: &str) -> bool {
    name.strip_suffix(".json").is_some_and(|stem| !stem.is_empty() && !stem.starts_with('.'))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaskFile {
    #[serde(default)]
    id: Option<serde_json::Value>,
    #[serde(default)]
    subject: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    active_form: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    blocked_by: Vec<serde_json::Value>,
    #[serde(default)]
    owner: Option<String>,
}

fn id_text(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_owned()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.filter(|s| !s.trim().is_empty())
}

fn task_entry(stem: &str, raw: &[u8]) -> Option<TodoEntry> {
    let task: TaskFile = serde_json::from_slice(raw).ok()?;
    let status = match task.status.as_deref() {
        Some("deleted") => return None,
        Some("completed") => "completed",
        Some("in_progress") => "in_progress",
        _ => "pending",
    };
    Some(TodoEntry {
        content: task.subject,
        status: status.to_owned(),
        active_form: non_empty(task.active_form),
        id: Some(task.id.as_ref().and_then(id_text).unwrap_or_else(|| stem.to_owned())),
        description: non_empty(task.description),
        blocked_by: task.blocked_by.iter().filter_map(id_text).collect(),
        owner: non_empty(task.owner),
    })
}

pub(super) fn read_task_list(dir: &Path) -> Vec<TodoEntry> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut list: Vec<TodoEntry> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let stem = name.strip_suffix(".json").filter(|_| is_task_file(&name))?;
            if e.metadata().ok()?.len() > MAX_TASK_FILE_BYTES {
                return None;
            }
            task_entry(stem, &std::fs::read(e.path()).ok()?)
        })
        .collect();
    list.sort_by_cached_key(|t| {
        let id = t.id.clone().unwrap_or_default();
        (id.parse::<u64>().unwrap_or(u64::MAX), id)
    });
    list
}

pub(super) fn tasks_root(projects_root: &Path) -> Option<PathBuf> {
    projects_root.parent().map(|config| config.join("tasks"))
}

impl Driver {
    pub(super) async fn scan_task_lists(&mut self, server_supports: bool) {
        let Some(root) = tasks_root(&self.cfg.projects_root).filter(|_| server_supports) else {
            self.task_watches.clear();
            return;
        };
        self.task_watches.retain(|short, _| self.transcript_locations.contains_key(short));
        let mut out = Vec::new();
        for (short, loc) in &self.transcript_locations {
            let dir = root.join(&loc.offset_key);
            let watch = self
                .task_watches
                .entry(short.clone())
                .and_modify(|w| w.retarget(&dir))
                .or_insert_with(|| TaskWatch::new(dir));
            if let Some(todos) = watch.poll() {
                out.push(AdapterEvent::Todos { local_id: loc.local_id.clone(), todos });
            }
        }
        for evt in out {
            self.emit(evt).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(name), body).unwrap();
    }

    fn real_task(id: &str, subject: &str, status: &str, blocked_by: &str) -> String {
        format!(
            r#"{{
  "id": "{id}",
  "subject": "{subject}",
  "description": "why {id}",
  "activeForm": "Doing {id}",
  "status": "{status}",
  "blocks": [],
  "blockedBy": [{blocked_by}]
}}"#
        )
    }

    #[test]
    fn real_task_files_map_onto_todo_entries_in_id_order() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write(dir, ".lock", "");
        write(dir, ".highwatermark", "11");
        write(dir, "10.json", &real_task("10", "ten", "in_progress", r#""2", "9""#));
        write(dir, "2.json", &real_task("2", "two", "completed", ""));
        write(dir, "9.json", &real_task("9", "nine", "pending", ""));
        write(
            dir,
            "11.json",
            r#"{"id":"11","subject":"eleven","description":"","status":"pending","blocks":[],"blockedBy":[],"owner":"agent-a"}"#,
        );
        write(dir, "12.json", &real_task("12", "gone", "deleted", ""));
        write(dir, "13.json", "{ torn");

        let list = read_task_list(dir);
        let ids: Vec<_> = list.iter().map(|t| t.id.as_deref().unwrap()).collect();
        assert_eq!(ids, ["2", "9", "10", "11"]);
        let ten = &list[2];
        assert_eq!(ten.content, "ten");
        assert_eq!(ten.status, "in_progress");
        assert_eq!(ten.active_form.as_deref(), Some("Doing 10"));
        assert_eq!(ten.description.as_deref(), Some("why 10"));
        assert_eq!(ten.blocked_by, ["2", "9"]);
        assert_eq!(ten.owner, None);
        let eleven = &list[3];
        assert_eq!(eleven.owner.as_deref(), Some("agent-a"));
        assert_eq!(eleven.description, None);
        assert_eq!(eleven.active_form, None);
    }

    #[test]
    fn a_change_is_sent_once_after_it_settles() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("sess");
        let mut watch = TaskWatch::new(dir.clone());
        assert_eq!(watch.poll(), None, "no folder");
        assert_eq!(watch.poll(), None, "a session without tasks sends nothing");

        write(&dir, "1.json", &real_task("1", "one", "pending", ""));
        assert_eq!(watch.poll(), None, "still settling");
        let sent = watch.poll().expect("settled");
        assert_eq!(sent.len(), 1);
        assert_eq!(watch.poll(), None, "unchanged is not resent");

        write(&dir, "2.json", &real_task("2", "two", "pending", ""));
        assert_eq!(watch.poll(), None);
        write(&dir, "3.json", &real_task("3", "three", "pending", ""));
        assert_eq!(watch.poll(), None, "a burst keeps settling");
        assert_eq!(watch.poll().expect("burst settled").len(), 3);

        for id in ["1", "2", "3"] {
            std::fs::remove_file(dir.join(format!("{id}.json"))).unwrap();
        }
        assert_eq!(watch.poll(), None);
        assert_eq!(watch.poll(), Some(Vec::new()), "deleting every task clears the list");
    }

    #[test]
    fn an_empty_folder_never_clears_a_list_it_did_not_send() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("sess");
        write(&dir, ".lock", "");
        let mut watch = TaskWatch::new(dir);
        assert_eq!(watch.poll(), None);
        assert_eq!(watch.poll(), None);
    }

    #[test]
    fn a_rotated_session_clears_the_list_it_had() {
        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join("old");
        write(&old, "1.json", &real_task("1", "one", "pending", ""));
        let mut watch = TaskWatch::new(old.clone());
        watch.poll();
        assert!(watch.poll().is_some());
        watch.retarget(&old);
        assert_eq!(watch.poll(), None, "same folder is not a rotation");

        watch.retarget(&tmp.path().join("new"));
        assert_eq!(watch.poll(), None);
        assert_eq!(watch.poll(), Some(Vec::new()));
    }

    #[test]
    fn the_tasks_root_sits_beside_the_projects_root() {
        assert_eq!(
            tasks_root(Path::new("/cfg/.claude/projects")),
            Some(PathBuf::from("/cfg/.claude/tasks"))
        );
    }
}
