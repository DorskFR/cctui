//! Re-prompts, then ends, a dispatched session whose turn is parked on a
//! background task whose completion notification never arrived.

use std::collections::BTreeMap;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use serde_json::Value;

/// Quiet time before the one re-prompt. Overridable via
/// `CCTUI_STUCK_TURN_REPROMPT_SECS`.
pub(super) const DEFAULT_REPROMPT_SECS: u64 = 600;

/// Quiet time after the re-prompt before the session is ended. Overridable via
/// `CCTUI_STUCK_TURN_END_SECS`.
pub(super) const DEFAULT_END_SECS: u64 = 600;

pub(super) const REPROMPT_TEXT: &str = "The background task you were waiting for has finished or failed. Check its output and continue your current step.";

pub(super) const END_DETAIL: &str =
    "stuck_waiting: no activity after the background-task re-prompt; ended to free the worker";

/// Most transcript bytes read in one scan, so a large backlog is caught up
/// over a few polls instead of in one blocking read.
const MAX_SCAN_BYTES: u64 = 8 * 1024 * 1024;

/// Text a tool result carries when a command or MCP call went to the
/// background; the task id follows each marker.
const START_MARKERS: [&str; 3] = [
    "running in background with ID: ",
    "moved to the background (ID: ",
    "moved to the background as task ",
];

pub(super) fn secs_from_env(var: Option<&str>, default: u64) -> Duration {
    Duration::from_secs(var.and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(default))
}

/// Background tasks started in a transcript and not yet notified.
///
/// Keyed by task id, with the task's output file when the transcript names it.
#[derive(Debug, Default)]
pub(super) struct BgTaskLedger {
    path: Option<PathBuf>,
    offset: u64,
    outstanding: BTreeMap<String, Option<PathBuf>>,
}

impl BgTaskLedger {
    /// Read the complete lines appended to `path` since the last scan.
    ///
    /// Returns whether any were read. A new path or a shrunk file restarts
    /// from zero.
    pub fn scan(&mut self, path: &Path) -> bool {
        if self.path.as_deref() != Some(path) {
            *self = Self { path: Some(path.to_path_buf()), ..Self::default() };
        }
        let Ok(mut file) = std::fs::File::open(path) else { return false };
        let Ok(len) = file.metadata().map(|m| m.len()) else { return false };
        if len < self.offset {
            *self = Self { path: Some(path.to_path_buf()), ..Self::default() };
        }
        if len == self.offset || file.seek(SeekFrom::Start(self.offset)).is_err() {
            return false;
        }
        let mut buf = Vec::new();
        if file.take(MAX_SCAN_BYTES).read_to_end(&mut buf).is_err() {
            return false;
        }
        let Some(end) = buf.iter().rposition(|b| *b == b'\n') else { return false };
        for line in buf[..end].split(|b| *b == b'\n') {
            if let Ok(line) = std::str::from_utf8(line) {
                self.feed_line(line);
            }
        }
        self.offset += end as u64 + 1;
        true
    }

    pub fn has_outstanding(&self) -> bool {
        !self.outstanding.is_empty()
    }

    /// Size and mtime of each outstanding task's output file.
    pub fn output_fingerprint(&self) -> Vec<(u64, Option<SystemTime>)> {
        self.outstanding
            .values()
            .flatten()
            .filter_map(|p| std::fs::metadata(p).ok())
            .map(|m| (m.len(), m.modified().ok()))
            .collect()
    }

    fn feed_line(&mut self, line: &str) {
        if line.contains("<task-notification>") {
            for id in notified_ids(line) {
                self.outstanding.remove(id);
            }
            return;
        }
        if !line.contains("background") {
            return;
        }
        let Ok(entry) = serde_json::from_str::<Value>(line) else { return };
        if entry.get("type").and_then(Value::as_str) != Some("user") {
            return;
        }
        let texts = tool_result_texts(&entry);
        let output = texts.iter().find_map(|t| output_path(t));
        let id = entry
            .pointer("/toolUseResult/backgroundTaskId")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| texts.iter().find_map(|t| started_id(t)));
        if let Some(id) = id.filter(|id| !id.is_empty()) {
            self.outstanding.insert(id, output);
        }
    }
}

pub fn notified_ids(line: &str) -> impl Iterator<Item = &str> {
    line.split("<task-id>")
        .skip(1)
        .filter_map(|rest| rest.split_once("</task-id>").map(|(id, _)| id.trim()))
}

fn tool_result_texts(entry: &Value) -> Vec<&str> {
    let Some(blocks) = entry.pointer("/message/content").and_then(Value::as_array) else {
        return Vec::new();
    };
    blocks
        .iter()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_result"))
        .filter_map(|b| b.get("content"))
        .flat_map(|c| match c {
            Value::String(s) => vec![s.as_str()],
            Value::Array(parts) => parts.iter().filter_map(|p| p.get("text")?.as_str()).collect(),
            _ => Vec::new(),
        })
        .collect()
}

pub fn started_id(text: &str) -> Option<String> {
    START_MARKERS.iter().find_map(|marker| {
        let rest = &text[text.find(marker)? + marker.len()..];
        let id: String = rest.chars().take_while(char::is_ascii_alphanumeric).collect();
        (!id.is_empty()).then_some(id)
    })
}

fn output_path(text: &str) -> Option<PathBuf> {
    const MARKER: &str = "Output is being written to: ";
    let rest = &text[text.find(MARKER)? + MARKER.len()..];
    let path = rest.split_whitespace().next()?.trim_end_matches('.');
    (!path.is_empty()).then(|| PathBuf::from(path))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StuckAction {
    Reprompt,
    End,
}

#[derive(Debug)]
pub(super) struct StuckTurnWatchdog {
    short: String,
    reprompt_after: Duration,
    end_after: Duration,
    pub ledger: BgTaskLedger,
    last_outputs: Vec<(u64, Option<SystemTime>)>,
    quiet_since: Option<Instant>,
    reprompted: bool,
    ended: bool,
}

impl StuckTurnWatchdog {
    pub fn new(session_id: &str, reprompt_after: Duration, end_after: Duration) -> Self {
        Self {
            short: session_id.chars().take(8).collect(),
            reprompt_after,
            end_after,
            ledger: BgTaskLedger::default(),
            last_outputs: Vec::new(),
            quiet_since: None,
            reprompted: false,
            ended: false,
        }
    }

    pub fn short(&self) -> &str {
        &self.short
    }

    /// Whether an outstanding task's output file changed since the last call.
    pub fn outputs_changed(&mut self) -> bool {
        let now = self.ledger.output_fingerprint();
        let changed = now != self.last_outputs;
        self.last_outputs = now;
        changed
    }

    /// Feed one poll; `active` is any sign of life.
    ///
    /// Before the re-prompt the quiet clock only runs while a background task
    /// is outstanding. After it any quiet counts: the lost notification keeps
    /// the task outstanding even once the agent has moved on.
    pub fn observe(&mut self, active: bool, now: Instant) -> Option<StuckAction> {
        if self.ended {
            return None;
        }
        if active || (!self.reprompted && !self.ledger.has_outstanding()) {
            self.quiet_since = None;
            return None;
        }
        let quiet = now.duration_since(*self.quiet_since.get_or_insert(now));
        if !self.reprompted && quiet >= self.reprompt_after {
            self.reprompted = true;
            self.quiet_since = Some(now);
            return Some(StuckAction::Reprompt);
        }
        if self.reprompted && quiet >= self.end_after {
            self.ended = true;
            return Some(StuckAction::End);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    const BASH_BG: &str = r#"{"type":"user","message":{"role":"user","content":[{"tool_use_id":"t1","type":"tool_result","content":"Command running in background with ID: b6s5c7kle. Output is being written to: /tmp/claude-1000/x/tasks/b6s5c7kle.output. You will be notified when it completes.","is_error":false}]},"toolUseResult":{"stdout":"","backgroundTaskId":"b6s5c7kle"}}"#;
    const TIMEOUT_BG: &str = r#"{"type":"user","message":{"role":"user","content":[{"tool_use_id":"t2","type":"tool_result","content":"Command did not complete within its 600s timeout and was moved to the background (ID: b7qxttdaw). Output is being written to: /tmp/o/b7qxttdaw.output. You will be notified when it completes."}]}}"#;
    const MCP_BG: &str = r#"{"type":"user","message":{"role":"user","content":[{"tool_use_id":"t3","type":"tool_result","content":[{"type":"text","text":"MCP tool \"cctui/CctuiAgent\" is still running after 120s. It was moved to the background as task kkdutsm9b and keeps running; you'll receive a notification with the result when it completes."}]}]},"toolUseResult":[{"type":"text","text":"moved to the background as task kkdutsm9b"}]}"#;
    const ASSISTANT: &str = r#"{"type":"assistant","message":{"role":"assistant","stop_reason":"end_turn","content":[{"type":"text","text":"Waiting for the background task to finish."}]}}"#;

    fn notification(id: &str) -> String {
        format!(
            r#"{{"type":"queue-operation","operation":"enqueue","content":"<task-notification>\n<task-id>{id}</task-id>\n<status>completed</status>\n</task-notification>"}}"#
        )
    }

    fn append(path: &Path, lines: &[&str]) {
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
        for line in lines {
            writeln!(f, "{line}").unwrap();
        }
    }

    fn watchdog() -> StuckTurnWatchdog {
        StuckTurnWatchdog::new(
            "abcd1234-0000-0000-0000-000000000000",
            Duration::from_mins(10),
            Duration::from_mins(10),
        )
    }

    #[test]
    fn ledger_tracks_every_start_shape_until_its_notification() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("s.jsonl");
        let mut ledger = BgTaskLedger::default();
        append(&path, &[BASH_BG, TIMEOUT_BG, MCP_BG, ASSISTANT]);
        assert!(ledger.scan(&path));
        let ids: Vec<_> = ledger.outstanding.keys().map(String::as_str).collect();
        assert_eq!(ids, ["b6s5c7kle", "b7qxttdaw", "kkdutsm9b"]);
        assert_eq!(
            ledger.outstanding["b6s5c7kle"].as_deref(),
            Some(Path::new("/tmp/claude-1000/x/tasks/b6s5c7kle.output"))
        );
        assert!(ledger.outstanding["kkdutsm9b"].is_none());

        assert!(!ledger.scan(&path), "nothing new");
        append(&path, &[&notification("b6s5c7kle"), &notification("kkdutsm9b")]);
        assert!(ledger.scan(&path));
        assert!(ledger.has_outstanding());
        append(&path, &[&notification("b7qxttdaw")]);
        ledger.scan(&path);
        assert!(!ledger.has_outstanding());
    }

    #[test]
    fn ledger_waits_for_a_complete_line_and_restarts_on_a_new_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("s.jsonl");
        std::fs::write(&path, &BASH_BG[..40]).unwrap();
        let mut ledger = BgTaskLedger::default();
        assert!(!ledger.scan(&path), "partial line is not consumed");
        std::fs::write(&path, format!("{BASH_BG}\n")).unwrap();
        assert!(ledger.scan(&path));
        assert!(ledger.has_outstanding());

        let moved = tmp.path().join("moved.jsonl");
        append(&moved, &[ASSISTANT]);
        ledger.scan(&moved);
        assert!(!ledger.has_outstanding(), "a new transcript path starts a fresh ledger");
    }

    #[test]
    fn assistant_text_quoting_a_marker_is_not_a_task() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("s.jsonl");
        append(
            &path,
            &[
                r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Command running in background with ID: fake1"}]}}"#,
            ],
        );
        let mut ledger = BgTaskLedger::default();
        ledger.scan(&path);
        assert!(!ledger.has_outstanding());
    }

    fn stuck(w: &mut StuckTurnWatchdog) {
        w.ledger.outstanding.insert("b1".to_owned(), None);
    }

    #[test]
    fn reprompts_once_then_ends_after_further_quiet() {
        let mut w = watchdog();
        stuck(&mut w);
        let t0 = Instant::now();
        assert_eq!(w.observe(false, t0), None);
        assert_eq!(w.observe(false, t0 + Duration::from_secs(599)), None);
        assert_eq!(w.observe(false, t0 + Duration::from_mins(10)), Some(StuckAction::Reprompt));
        assert_eq!(w.observe(false, t0 + Duration::from_secs(1199)), None);
        assert_eq!(w.observe(false, t0 + Duration::from_mins(20)), Some(StuckAction::End));
        assert_eq!(w.observe(false, t0 + Duration::from_hours(2)), None, "ends exactly once");
    }

    #[test]
    fn activity_after_the_reprompt_restarts_the_end_clock_without_a_second_reprompt() {
        let mut w = watchdog();
        stuck(&mut w);
        let t0 = Instant::now();
        w.observe(false, t0);
        assert_eq!(w.observe(false, t0 + Duration::from_mins(10)), Some(StuckAction::Reprompt));
        assert_eq!(w.observe(true, t0 + Duration::from_mins(11)), None, "the agent answered");
        assert_eq!(w.observe(false, t0 + Duration::from_mins(12)), None);
        assert_eq!(w.observe(false, t0 + Duration::from_secs(1319)), None);
        assert_eq!(w.observe(false, t0 + Duration::from_mins(22)), Some(StuckAction::End));
    }

    #[test]
    fn quiet_without_an_outstanding_task_never_fires() {
        let mut w = watchdog();
        let t0 = Instant::now();
        assert_eq!(w.observe(false, t0), None);
        assert_eq!(w.observe(false, t0 + Duration::from_hours(3)), None);
    }

    #[test]
    fn activity_before_the_threshold_resets_the_reprompt_clock() {
        let mut w = watchdog();
        stuck(&mut w);
        let t0 = Instant::now();
        w.observe(false, t0);
        assert_eq!(w.observe(true, t0 + Duration::from_mins(9)), None);
        assert_eq!(w.observe(false, t0 + Duration::from_mins(10)), None);
        assert_eq!(w.observe(false, t0 + Duration::from_secs(1199)), None);
        assert_eq!(w.observe(false, t0 + Duration::from_mins(20)), Some(StuckAction::Reprompt));
    }

    #[test]
    fn a_growing_task_output_counts_as_activity() {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("b1.output");
        std::fs::write(&out, "a").unwrap();
        let mut w = watchdog();
        w.ledger.outstanding.insert("b1".to_owned(), Some(out.clone()));
        assert!(w.outputs_changed(), "first sighting");
        assert!(!w.outputs_changed());
        std::fs::write(&out, "ab").unwrap();
        assert!(w.outputs_changed());
    }

    #[test]
    fn env_parsing() {
        assert_eq!(secs_from_env(None, DEFAULT_REPROMPT_SECS), Duration::from_mins(10));
        assert_eq!(secs_from_env(Some(" 30 "), DEFAULT_END_SECS), Duration::from_secs(30));
        assert_eq!(secs_from_env(Some("x"), DEFAULT_END_SECS), Duration::from_mins(10));
    }
}
