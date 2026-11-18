//! Turn ends the roster does not report: a worker without cctui's hooks fires
//! no `Stop` hook, and a worker adopted by a restarted supervisor is listed as
//! active whatever it does, so the transcript decides.

use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::Path;
use std::time::{Duration, SystemTime};

use serde_json::Value;

use super::{Driver, LiveSnapshot};

const TAIL_BYTES: u64 = 64 * 1024;

/// A reply typed into the PTY reaches the transcript a moment after the roster
/// turns busy; until then the previous turn's end is still the last entry.
const QUIET: Duration = Duration::from_secs(5);

/// Keyed by the last main-chain assistant entry, so a later `turn_duration`
/// does not count as a second turn end.
fn ended_turn_in(tail: &str) -> Option<String> {
    let uuid = |v: &Value| v.get("uuid").and_then(Value::as_str).map(str::to_owned);
    let mut duration = None;
    for line in tail.lines().rev() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if v.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        match v.get("type").and_then(Value::as_str) {
            Some("system") => {
                if duration.is_none()
                    && v.get("subtype").and_then(Value::as_str) == Some("turn_duration")
                {
                    duration = Some(uuid(&v));
                }
            }
            Some("assistant") => {
                let stop = v.pointer("/message/stop_reason").and_then(Value::as_str);
                return (duration.is_some() || stop == Some("end_turn")).then(|| uuid(&v))?;
            }
            Some("user") => return duration.flatten(),
            _ => {}
        }
    }
    duration.flatten()
}

/// The transcript as last read, so an unchanged file is not read again.
pub(super) struct TranscriptRead {
    modified: SystemTime,
    len: u64,
    turn: Option<String>,
}

fn ended_turn(path: &Path, now: SystemTime, last: &mut Option<TranscriptRead>) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    if now.duration_since(modified).ok().is_none_or(|q| q < QUIET) {
        return None;
    }
    if let Some(read) = last
        && read.modified == modified
        && read.len == meta.len()
    {
        return read.turn.clone();
    }
    let mut file = std::fs::File::open(path).ok()?;
    file.seek(SeekFrom::Start(meta.len().saturating_sub(TAIL_BYTES))).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let turn = ended_turn_in(&String::from_utf8_lossy(&bytes));
    *last = Some(TranscriptRead { modified, len: meta.len(), turn: turn.clone() });
    turn
}

/// What the classifier would bucket as Working.
fn reads_busy(job: &LiveSnapshot) -> bool {
    job.tempo.as_deref() != Some("blocked")
        && (job.tempo.as_deref() == Some("active")
            || matches!(job.state.as_deref(), Some("working" | "running")))
}

impl Driver {
    /// Whether `job` reads busy though its transcript shows the turn ended.
    /// Signals each turn end of a hookless worker once, busy or not.
    pub(super) async fn transcript_turn_ended(
        &mut self,
        job: &LiveSnapshot,
        local_id: &str,
    ) -> bool {
        let hookless = super::super::canary::hooks_missing(local_id);
        let adopted = job.state.as_deref() == Some("adopted") && reads_busy(job);
        if !(hookless || adopted) || self.lanes.is_busy(local_id) {
            return false;
        }
        let Some(path) = self.transcript_locations.get(&job.short).map(|l| l.path.clone()) else {
            return false;
        };
        let last = self.transcript_reads.entry(local_id.to_owned()).or_default();
        let Some(turn) = ended_turn(&path, SystemTime::now(), last) else { return false };
        if hookless && self.hookless_turn_ends.get(local_id) != Some(&turn) {
            tracing::info!(%local_id, short = %job.short, %turn, "turn end of a hookless worker read from its transcript");
            self.hookless_turn_ends.insert(local_id.to_owned(), turn);
            crate::adapters::turn_end::emit(&self.events, local_id).await;
        }
        reads_busy(job)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const USER: &str =
        r#"{"type":"user","uuid":"u1","isSidechain":false,"message":{"content":"go"}}"#;
    const TOOL: &str = r#"{"type":"assistant","uuid":"a1","isSidechain":false,"message":{"stop_reason":"tool_use"}}"#;
    const RESULT: &str = r#"{"type":"user","uuid":"u2","isSidechain":false,"message":{"content":[{"type":"tool_result"}]}}"#;
    const END: &str = r#"{"type":"assistant","uuid":"a2","isSidechain":false,"message":{"stop_reason":"end_turn"}}"#;
    const DURATION: &str = r#"{"type":"system","subtype":"turn_duration","uuid":"s1","isSidechain":false,"durationMs":5}"#;
    const META: &str = r#"{"type":"cost-state","sessionId":"x"}"#;

    fn tail(lines: &[&str]) -> String {
        lines.join("\n") + "\n"
    }

    #[test]
    fn an_end_turn_or_turn_duration_with_nothing_after_ends_the_turn() {
        assert_eq!(ended_turn_in(&tail(&[USER, TOOL, RESULT, END])).as_deref(), Some("a2"));
        let full = tail(&[USER, END, DURATION, META, r#"{"type":"last-prompt"}"#]);
        assert_eq!(
            ended_turn_in(&full).as_deref(),
            Some("a2"),
            "metadata after the end is skipped"
        );
    }

    #[test]
    fn a_turn_duration_keeps_the_key_of_the_turn_it_closes() {
        let before = ended_turn_in(&tail(&[USER, END]));
        let after = ended_turn_in(&tail(&[USER, END, DURATION]));
        assert_eq!(before, after, "one turn end, signalled once");
    }

    #[test]
    fn a_running_turn_has_not_ended() {
        assert_eq!(ended_turn_in(&tail(&[USER])), None);
        assert_eq!(ended_turn_in(&tail(&[USER, TOOL])), None);
        assert_eq!(ended_turn_in(&tail(&[USER, TOOL, RESULT])), None);
        assert_eq!(ended_turn_in(&tail(&[USER, END, USER])), None, "a new prompt started");
        assert_eq!(ended_turn_in(""), None);
    }

    #[test]
    fn sidechain_entries_and_a_cut_first_line_do_not_decide() {
        let side = r#"{"type":"user","uuid":"x","isSidechain":true,"message":{"content":"sub"}}"#;
        assert_eq!(ended_turn_in(&tail(&[END, side])).as_deref(), Some("a2"));
        let cut = format!("e\":\"tool_use\"}}}}\n{END}\n");
        assert_eq!(ended_turn_in(&cut).as_deref(), Some("a2"));
    }

    #[test]
    fn a_transcript_still_being_written_is_not_read_as_ended() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        std::fs::write(&path, tail(&[USER, END])).unwrap();
        let written = std::fs::metadata(&path).unwrap().modified().unwrap();
        let mut last = None;
        assert_eq!(ended_turn(&path, written, &mut last), None);
        assert!(last.is_none(), "a transcript still settling is read again");
        assert_eq!(ended_turn(&path, written + QUIET, &mut last).as_deref(), Some("a2"));
        assert_eq!(ended_turn(&dir.path().join("missing.jsonl"), written + QUIET, &mut None), None);
    }

    #[test]
    fn an_unchanged_transcript_is_not_read_twice() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        std::fs::write(&path, tail(&[USER, END])).unwrap();
        let written = std::fs::metadata(&path).unwrap().modified().unwrap();
        let mut last = None;
        assert_eq!(ended_turn(&path, written + QUIET, &mut last).as_deref(), Some("a2"));
        last.as_mut().unwrap().turn = Some("cached".to_owned());
        assert_eq!(ended_turn(&path, written + QUIET, &mut last).as_deref(), Some("cached"));

        std::fs::write(&path, tail(&[USER, END, USER])).unwrap();
        let rewritten = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(ended_turn(&path, rewritten + QUIET, &mut last), None, "a grown file is read");
    }

    fn job(tempo: &str, state: &str) -> LiveSnapshot {
        serde_json::from_value(
            serde_json::json!({"short": "abcd1234", "tempo": tempo, "state": state}),
        )
        .unwrap()
    }

    #[test]
    fn only_a_roster_that_reads_busy_needs_the_transcript() {
        assert!(reads_busy(&job("active", "working")));
        assert!(reads_busy(&job("idle", "working")));
        assert!(!reads_busy(&job("idle", "done")));
        assert!(!reads_busy(&job("blocked", "working")), "a blocked worker is Needs input");
    }

    fn statuses(
        rx: &mut tokio::sync::mpsc::Receiver<super::super::AdapterEvent>,
    ) -> Vec<(Option<String>, Option<String>)> {
        let mut out = Vec::new();
        while let Ok(evt) = rx.try_recv() {
            if let super::super::AdapterEvent::Status { tempo, state, .. } = evt {
                out.push((tempo, state));
            }
        }
        out
    }

    #[tokio::test]
    async fn a_hookless_worker_whose_turn_ended_leaves_working() {
        use super::super::test_support::{driver, snap, write_main_transcript};
        let (mut d, mut rx) = driver();
        let local_id = write_main_transcript(&d, "f00d0001", &[USER, END, DURATION]);
        let path =
            super::super::transcript::transcript_path(&d.cfg.projects_root, "/tmp", &local_id);
        let quiet = SystemTime::now() - QUIET * 2;
        std::fs::File::options().append(true).open(&path).unwrap().set_modified(quiet).unwrap();

        d.apply_snapshot(vec![snap("f00d0001", "working", None)]).await;
        assert_eq!(
            statuses(&mut rx),
            [(Some("active".to_owned()), Some("working".to_owned()))],
            "hooks are not known missing yet"
        );

        super::super::super::canary::flag_hooks_missing(&local_id);
        d.apply_snapshot(vec![snap("f00d0001", "working", None)]).await;
        assert_eq!(statuses(&mut rx), [(Some("idle".to_owned()), Some("done".to_owned()))]);
        assert_eq!(d.hookless_turn_ends.get(&local_id).map(String::as_str), Some("a2"));

        d.apply_snapshot(vec![snap("f00d0001", "working", None)]).await;
        assert!(statuses(&mut rx).is_empty(), "the recovered turn end is reported once");
    }

    fn settled_transcript(d: &Driver, short: &str, lines: &[&str]) -> (String, std::path::PathBuf) {
        use super::super::test_support::write_main_transcript;
        let local_id = write_main_transcript(d, short, lines);
        let path =
            super::super::transcript::transcript_path(&d.cfg.projects_root, "/tmp", &local_id);
        let quiet = SystemTime::now() - QUIET * 2;
        std::fs::File::options().append(true).open(&path).unwrap().set_modified(quiet).unwrap();
        (local_id, path)
    }

    #[tokio::test]
    async fn a_hookless_worker_the_roster_reads_idle_still_signals_its_turn_end() {
        use super::super::test_support::{driver, snap};
        let (mut d, _rx) = driver();
        let result = r#"{"type":"assistant","uuid":"a9","isSidechain":false,"message":{"stop_reason":"end_turn","content":[{"type":"text","text":"result: x"}]}}"#;
        let (local_id, _) = settled_transcript(&d, "f00d0002", &[USER, result]);
        super::super::super::canary::flag_hooks_missing(&local_id);

        let mut idle = snap("f00d0002", "done", None);
        idle.tempo = Some("idle".to_owned());
        d.apply_snapshot(vec![idle]).await;
        assert_eq!(
            d.hookless_turn_ends.get(&local_id).map(String::as_str),
            Some("a9"),
            "one snapshot is enough"
        );
    }

    #[tokio::test]
    async fn an_adopted_worker_whose_turn_ended_leaves_working() {
        use super::super::test_support::{driver, snap};
        let (mut d, mut rx) = driver();
        let (_, path) = settled_transcript(&d, "f00d0003", &[USER, END, DURATION]);

        d.apply_snapshot(vec![snap("f00d0003", "adopted", None)]).await;
        assert_eq!(statuses(&mut rx), [(Some("idle".to_owned()), Some("done".to_owned()))]);
        assert!(d.hookless_turn_ends.is_empty(), "its Stop hook signals the turn end");

        std::fs::write(&path, tail(&[USER, END, DURATION, USER, TOOL])).unwrap();
        let quiet = SystemTime::now() - QUIET * 2;
        std::fs::File::options().append(true).open(&path).unwrap().set_modified(quiet).unwrap();
        d.apply_snapshot(vec![snap("f00d0003", "adopted", None)]).await;
        assert_eq!(
            statuses(&mut rx),
            [(Some("active".to_owned()), Some("adopted".to_owned()))],
            "a turn in flight is working"
        );
    }
}
