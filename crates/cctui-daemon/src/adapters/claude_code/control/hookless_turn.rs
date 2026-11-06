//! Turn ends of a worker without cctui's hooks: no `Stop` hook fires and the
//! roster can stay busy after the turn, so the transcript decides.

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

fn ended_turn(path: &Path, now: SystemTime) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let meta = file.metadata().ok()?;
    let quiet = meta.modified().ok().and_then(|m| now.duration_since(m).ok());
    if quiet.is_none_or(|q| q < QUIET) {
        return None;
    }
    file.seek(SeekFrom::Start(meta.len().saturating_sub(TAIL_BYTES))).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    ended_turn_in(&String::from_utf8_lossy(&bytes))
}

/// What the classifier would bucket as Working.
fn reads_busy(job: &LiveSnapshot) -> bool {
    job.tempo.as_deref() != Some("blocked")
        && (job.tempo.as_deref() == Some("active")
            || matches!(job.state.as_deref(), Some("working" | "running")))
}

impl Driver {
    /// Whether `job` reads busy only because its hooks never loaded: its
    /// transcript shows the turn ended. Signals each such turn end once.
    pub(super) async fn hookless_turn_ended(&mut self, job: &LiveSnapshot, local_id: &str) -> bool {
        if !reads_busy(job)
            || self.lanes.is_busy(local_id)
            || !super::super::canary::hooks_missing(local_id)
        {
            return false;
        }
        let Some(path) = self.transcript_locations.get(&job.short).map(|l| l.path.clone()) else {
            return false;
        };
        let Some(turn) = ended_turn(&path, SystemTime::now()) else { return false };
        if self.hookless_turn_ends.get(local_id) != Some(&turn) {
            tracing::info!(%local_id, short = %job.short, %turn, "turn end of a hookless worker read from its transcript");
            self.hookless_turn_ends.insert(local_id.to_owned(), turn);
            crate::adapters::turn_end::emit(&self.events, local_id).await;
        }
        true
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
        assert_eq!(ended_turn(&path, written), None);
        assert_eq!(ended_turn(&path, written + QUIET).as_deref(), Some("a2"));
        assert_eq!(ended_turn(&dir.path().join("missing.jsonl"), written + QUIET), None);
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
}
