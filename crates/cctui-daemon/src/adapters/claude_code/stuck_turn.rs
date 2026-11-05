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

/// Longest quiet stretch a live Monitor holds the clock for. Overridable via
/// `CCTUI_STUCK_TURN_CEILING_SECS`.
pub(super) const DEFAULT_CEILING_SECS: u64 = 7200;

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

const MONITOR_MARKER: &str = "Monitor started (task ";
const ASYNC_AGENT_MARKER: &str = "Async agent launched";
const STOP_MARKERS: [&str; 2] = ["Successfully stopped task: ", "Successfully killed shell: "];

pub(super) fn secs_from_env(var: Option<&str>, default: u64) -> Duration {
    Duration::from_secs(var.and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(default))
}

/// When an outstanding task stops running on its own.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Expiry {
    /// No signal: a quiet task may be one whose notification was lost.
    #[default]
    Unknown,
    At(SystemTime),
    Never,
}

#[derive(Debug, Clone, Default)]
struct BgTask {
    output: Option<PathBuf>,
    expiry: Expiry,
}

/// Background tasks started in a transcript and not yet notified.
#[derive(Debug, Default)]
pub(super) struct BgTaskLedger {
    path: Option<PathBuf>,
    offset: u64,
    outstanding: BTreeMap<String, BgTask>,
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

    /// Whether an outstanding task is known to still be running at `now`.
    pub fn holds_turn(&self, now: SystemTime) -> bool {
        self.outstanding.values().any(|t| match t.expiry {
            Expiry::Never => true,
            Expiry::At(at) => now < at,
            Expiry::Unknown => false,
        })
    }

    /// Size and mtime of each outstanding task's output file.
    pub fn output_fingerprint(&self) -> Vec<(u64, Option<SystemTime>)> {
        self.outstanding
            .values()
            .filter_map(|t| t.output.as_deref())
            .filter_map(|p| std::fs::metadata(p).ok())
            .map(|m| (m.len(), m.modified().ok()))
            .collect()
    }

    fn feed_line(&mut self, line: &str) {
        if line.contains("<task-notification>") {
            for id in terminal_ids(line) {
                self.outstanding.remove(id);
            }
            return;
        }
        if !line.contains("background")
            && !line.contains(MONITOR_MARKER)
            && !STOP_MARKERS.iter().any(|m| line.contains(m))
        {
            return;
        }
        let Ok(entry) = serde_json::from_str::<Value>(line) else { return };
        if entry.get("type").and_then(Value::as_str) != Some("user") {
            return;
        }
        let texts = tool_result_texts(&entry);
        for id in texts.iter().filter_map(|t| stopped_id(t)) {
            self.outstanding.remove(id);
        }
        let output = texts.iter().find_map(|t| output_path(t));
        let id = entry
            .pointer("/toolUseResult/backgroundTaskId")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| texts.iter().find_map(|t| started_id(t)));
        if let Some(id) = id.filter(|id| !id.is_empty()) {
            let expiry = if texts.iter().any(|t| is_monitor_start(t)) {
                monitor_expiry(&entry)
            } else {
                Expiry::Unknown
            };
            self.outstanding.insert(id, BgTask { output, expiry });
        }
    }
}

/// When a Monitor stops: `timeoutMs` after its start line, never if `persistent`.
fn monitor_expiry(entry: &Value) -> Expiry {
    let result = entry.get("toolUseResult");
    if result.and_then(|r| r.get("persistent")).and_then(Value::as_bool) == Some(true) {
        return Expiry::Never;
    }
    let Some(ms) = result.and_then(|r| r.get("timeoutMs")).and_then(Value::as_u64) else {
        return Expiry::Unknown;
    };
    let started = entry
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .map_or_else(SystemTime::now, SystemTime::from);
    Expiry::At(started + Duration::from_millis(ms))
}

/// Each `<task-notification>` in `line`: its task id, and whether it ends it.
///
/// A Monitor also notifies each event it sees, without a `<status>`, while it
/// keeps watching.
pub fn task_notices(line: &str) -> impl Iterator<Item = (&str, bool)> {
    line.split("<task-notification>").skip(1).filter_map(|block| {
        let block = block.split("</task-notification>").next().unwrap_or(block);
        let rest = block.split_once("<task-id>")?.1;
        let id = rest.split_once("</task-id>")?.0.trim();
        Some((id, block.contains("<status>")))
    })
}

fn terminal_ids(line: &str) -> impl Iterator<Item = &str> {
    task_notices(line).filter_map(|(id, terminal)| terminal.then_some(id))
}

/// The task a tool result sent to the background.
///
/// A command or MCP call, a Monitor watch, or a background Agent. A foreground
/// Agent's result names its `agentId` too, so only a launch counts.
pub fn started_id(text: &str) -> Option<String> {
    START_MARKERS
        .iter()
        .find_map(|marker| id_after(text, marker))
        .or_else(|| id_after(text, MONITOR_MARKER))
        .or_else(|| id_after(text, "agentId: ").filter(|_| text.contains(ASYNC_AGENT_MARKER)))
}

pub fn is_monitor_start(text: &str) -> bool {
    text.contains(MONITOR_MARKER)
}

/// The background task a stop result reports stopped.
pub fn stopped_id(text: &str) -> Option<&str> {
    STOP_MARKERS.iter().find_map(|marker| {
        let rest = &text[text.find(marker)? + marker.len()..];
        let end = rest.find(|c: char| !c.is_ascii_alphanumeric()).unwrap_or(rest.len());
        (end > 0).then(|| &rest[..end])
    })
}

fn id_after(text: &str, marker: &str) -> Option<String> {
    let rest = &text[text.find(marker)? + marker.len()..];
    let id: String = rest.chars().take_while(char::is_ascii_alphanumeric).collect();
    (!id.is_empty()).then_some(id)
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
    ceiling: Duration,
    pub ledger: BgTaskLedger,
    last_outputs: Vec<(u64, Option<SystemTime>)>,
    quiet_since: Option<Instant>,
    held_since: Option<Instant>,
    reprompted: bool,
    ended: bool,
}

impl StuckTurnWatchdog {
    pub fn new(session_id: &str, reprompt_after: Duration, end_after: Duration) -> Self {
        Self {
            short: session_id.chars().take(8).collect(),
            reprompt_after,
            end_after,
            ceiling: Duration::from_secs(DEFAULT_CEILING_SECS),
            ledger: BgTaskLedger::default(),
            last_outputs: Vec::new(),
            quiet_since: None,
            held_since: None,
            reprompted: false,
            ended: false,
        }
    }

    #[must_use]
    pub const fn with_ceiling(mut self, ceiling: Duration) -> Self {
        self.ceiling = ceiling;
        self
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

    pub fn observe(&mut self, active: bool, now: Instant) -> Option<StuckAction> {
        self.observe_at(active, now, SystemTime::now())
    }

    /// Feed one poll; `active` is any sign of life, `wall` dates task expiry.
    ///
    /// Before the re-prompt the quiet clock only runs while a background task
    /// is outstanding. After it any quiet counts: the lost notification keeps
    /// the task outstanding even once the agent has moved on. A task known to
    /// be running holds the clock, for at most the ceiling per quiet stretch.
    pub fn observe_at(
        &mut self,
        active: bool,
        now: Instant,
        wall: SystemTime,
    ) -> Option<StuckAction> {
        if self.ended {
            return None;
        }
        if active || (!self.reprompted && !self.ledger.has_outstanding()) {
            self.quiet_since = None;
            self.held_since = None;
            return None;
        }
        if self.ledger.holds_turn(wall) {
            if now.duration_since(*self.held_since.get_or_insert(now)) < self.ceiling {
                self.quiet_since = None;
                return None;
            }
        } else {
            self.held_since = None;
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
            ledger.outstanding["b6s5c7kle"].output.as_deref(),
            Some(Path::new("/tmp/claude-1000/x/tasks/b6s5c7kle.output"))
        );
        assert!(ledger.outstanding["kkdutsm9b"].output.is_none());
        assert!(!ledger.holds_turn(SystemTime::now()), "no expiry known for these");

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

    const MONITOR_START: &str = r#"{"type":"user","message":{"role":"user","content":[{"tool_use_id":"tm","type":"tool_result","content":"Monitor started (task b3bzv1ch3, expires in 30m unless the source ends first; you get one notice at expiry). You will be notified on each event."}]},"toolUseResult":{"taskId":"b3bzv1ch3","timeoutMs":1800000,"persistent":false}}"#;
    const ASYNC_AGENT: &str = r#"{"type":"user","message":{"role":"user","content":[{"tool_use_id":"ta","type":"tool_result","content":[{"type":"text","text":"Async agent launched successfully.\nagentId: a516e4c3eb9cc21fd (internal ID)\nThe agent is working in the background. You will be notified automatically when it completes."}]}]}}"#;
    const FOREGROUND_AGENT: &str = r#"{"type":"user","message":{"role":"user","content":[{"tool_use_id":"tf","type":"tool_result","content":[{"type":"text","text":"The tests run in the background job.\nagentId: a0000fore (use SendMessage to continue this agent)"}]}]}}"#;

    fn monitor_event(id: &str) -> String {
        format!(
            r#"{{"type":"queue-operation","operation":"enqueue","content":"<task-notification>\n<task-id>{id}</task-id>\n<summary>Monitor event: \"CI\"</summary>\n<event>pending</event>\n</task-notification>"}}"#
        )
    }

    #[test]
    fn ledger_keeps_a_monitor_through_its_events_until_the_stream_ends() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("s.jsonl");
        let mut ledger = BgTaskLedger::default();
        append(&path, &[MONITOR_START, ASSISTANT]);
        ledger.scan(&path);
        assert_eq!(ledger.outstanding.keys().collect::<Vec<_>>(), ["b3bzv1ch3"]);

        append(&path, &[&monitor_event("b3bzv1ch3"), &monitor_event("b3bzv1ch3")]);
        ledger.scan(&path);
        assert!(ledger.has_outstanding(), "an event notice is not the end of the watch");

        append(&path, &[&notification("b3bzv1ch3")]);
        ledger.scan(&path);
        assert!(!ledger.has_outstanding());
    }

    #[test]
    fn ledger_tracks_a_background_agent_but_not_a_foreground_one() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("s.jsonl");
        let mut ledger = BgTaskLedger::default();
        append(&path, &[FOREGROUND_AGENT, ASYNC_AGENT]);
        ledger.scan(&path);
        assert_eq!(ledger.outstanding.keys().collect::<Vec<_>>(), ["a516e4c3eb9cc21fd"]);
        append(&path, &[&notification("a516e4c3eb9cc21fd")]);
        ledger.scan(&path);
        assert!(!ledger.has_outstanding());
    }

    #[test]
    fn terminal_ids_skip_status_less_notices() {
        let line = "<task-notification><task-id>m1</task-id><event>x</event></task-notification>\
                    <task-notification><task-id>b2</task-id><status>failed</status></task-notification>";
        assert_eq!(terminal_ids(line).collect::<Vec<_>>(), ["b2"]);
        assert_eq!(task_notices(line).collect::<Vec<_>>(), [("m1", false), ("b2", true)]);
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
        w.ledger.outstanding.insert("b1".to_owned(), BgTask::default());
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
        w.ledger
            .outstanding
            .insert("b1".to_owned(), BgTask { output: Some(out.clone()), ..BgTask::default() });
        assert!(w.outputs_changed(), "first sighting");
        assert!(!w.outputs_changed());
        std::fs::write(&out, "ab").unwrap();
        assert!(w.outputs_changed());
    }

    fn monitor(w: &mut StuckTurnWatchdog, expiry: Expiry) {
        w.ledger.outstanding.insert("m1".to_owned(), BgTask { output: None, expiry });
    }

    #[test]
    fn a_live_monitor_holds_the_watchdog_until_the_ceiling() {
        let mut w = watchdog().with_ceiling(Duration::from_hours(2));
        monitor(&mut w, Expiry::Never);
        let (t0, wall) = (Instant::now(), SystemTime::now());
        assert_eq!(w.observe_at(false, t0, wall), None);
        assert_eq!(w.observe_at(false, t0 + Duration::from_mins(25), wall), None, "no re-prompt");
        assert_eq!(w.observe_at(false, t0 + Duration::from_mins(119), wall), None, "no end");
        assert_eq!(w.observe_at(false, t0 + Duration::from_hours(2), wall), None);
        assert_eq!(
            w.observe_at(false, t0 + Duration::from_mins(130), wall),
            Some(StuckAction::Reprompt),
            "past the ceiling the normal clock runs"
        );
        assert_eq!(
            w.observe_at(false, t0 + Duration::from_mins(140), wall),
            Some(StuckAction::End),
            "and still ends the turn"
        );
    }

    #[test]
    fn activity_restarts_the_ceiling() {
        let mut w = watchdog().with_ceiling(Duration::from_hours(1));
        monitor(&mut w, Expiry::Never);
        let (t0, wall) = (Instant::now(), SystemTime::now());
        w.observe_at(false, t0, wall);
        assert_eq!(w.observe_at(true, t0 + Duration::from_mins(50), wall), None, "a monitor event");
        assert_eq!(w.observe_at(false, t0 + Duration::from_mins(51), wall), None);
        assert_eq!(w.observe_at(false, t0 + Duration::from_mins(100), wall), None, "held again");
    }

    #[test]
    fn a_monitor_past_its_expiry_no_longer_holds() {
        let mut w = watchdog();
        let (t0, wall) = (Instant::now(), SystemTime::now());
        monitor(&mut w, Expiry::At(wall + Duration::from_mins(30)));
        let at = |mins| (t0 + Duration::from_mins(mins), wall + Duration::from_mins(mins));
        assert_eq!(w.observe_at(false, t0, wall), None);
        let (now, then) = at(25);
        assert_eq!(w.observe_at(false, now, then), None, "still live at 25 min");
        let (now, then) = at(30);
        assert_eq!(w.observe_at(false, now, then), None, "the quiet clock starts at expiry");
        let (now, then) = at(40);
        assert_eq!(w.observe_at(false, now, then), Some(StuckAction::Reprompt));
    }

    const MONITOR_DATED: &str = r#"{"type":"user","timestamp":"2026-01-01T00:00:00Z","message":{"role":"user","content":[{"tool_use_id":"tm","type":"tool_result","content":"Monitor started (task mdated01, expires in 30m unless the source ends first; you get one notice at expiry). You will be notified on each event."}]},"toolUseResult":{"taskId":"mdated01","timeoutMs":1800000,"persistent":false}}"#;
    const MONITOR_PERSISTENT: &str = r#"{"type":"user","message":{"role":"user","content":[{"tool_use_id":"tp","type":"tool_result","content":"Monitor started (task mpers001). You will be notified on each event."}]},"toolUseResult":{"taskId":"mpers001","persistent":true}}"#;
    const TASK_STOP: &str = r#"{"type":"user","message":{"role":"user","content":[{"tool_use_id":"ts","type":"tool_result","content":"{\"message\":\"Successfully stopped task: mpers001 (watch CI)\",\"task_id\":\"mpers001\"}"}]}}"#;

    fn utc(s: &str) -> SystemTime {
        chrono::DateTime::parse_from_rfc3339(s).unwrap().into()
    }

    #[test]
    fn ledger_dates_a_monitor_from_its_start_line() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("s.jsonl");
        let mut ledger = BgTaskLedger::default();
        append(&path, &[MONITOR_DATED]);
        ledger.scan(&path);
        assert!(ledger.holds_turn(utc("2026-01-01T00:29:00Z")));
        assert!(!ledger.holds_turn(utc("2026-01-01T00:31:00Z")));
        assert!(ledger.has_outstanding(), "expiry alone does not clear the task");
    }

    #[test]
    fn ledger_holds_a_persistent_monitor_until_it_is_stopped() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("s.jsonl");
        let mut ledger = BgTaskLedger::default();
        append(&path, &[MONITOR_PERSISTENT]);
        ledger.scan(&path);
        assert!(ledger.holds_turn(SystemTime::now() + Duration::from_hours(48)));
        append(&path, &[TASK_STOP]);
        ledger.scan(&path);
        assert!(!ledger.has_outstanding(), "a TaskStop result clears it");
    }

    #[test]
    fn started_id_covers_every_background_launch() {
        assert_eq!(
            started_id("Monitor started (task b3bzv1ch3, expires").as_deref(),
            Some("b3bzv1ch3")
        );
        assert_eq!(
            started_id("Async agent launched successfully.\nagentId: a516e4c3 (internal ID)")
                .as_deref(),
            Some("a516e4c3")
        );
        assert_eq!(started_id("agentId: a0000fore (use SendMessage)"), None);
        assert_eq!(stopped_id("Successfully killed shell: bash1 (sleep 99)"), Some("bash1"));
    }

    #[test]
    fn env_parsing() {
        assert_eq!(secs_from_env(None, DEFAULT_REPROMPT_SECS), Duration::from_mins(10));
        assert_eq!(secs_from_env(Some(" 30 "), DEFAULT_END_SECS), Duration::from_secs(30));
        assert_eq!(secs_from_env(Some("x"), DEFAULT_END_SECS), Duration::from_mins(10));
    }
}
