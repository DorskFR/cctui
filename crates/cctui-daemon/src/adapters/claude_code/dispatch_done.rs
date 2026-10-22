//! Turn-complete marker for dispatch-originated sessions.
//!
//! A dispatched worker pod's entrypoint (deploy/worker-entrypoint.sh,
//! `await_dispatch_done`) blocks on done/crash signals — but a session that
//! finishes its turn and just sits idle-and-alive under `claude daemon` fires
//! none of them (no guard step=-1, no `RESULT_FILE`, the daemon stays up, the
//! session stays registered), so the pod lingered to `activeDeadlineSeconds`.
//!
//! This tracker watches the one session `maybe_dispatch_on_start` launched
//! and, once it has been seen busy at least once and then stayed non-busy for
//! a settle window, writes `<jobs_root>/<short>/dispatch_done`. The entrypoint
//! polls for that marker and winds the pod down within seconds. Only
//! dispatch-originated sessions ever construct a tracker, so interactive
//! sessions can never get a marker file.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cctui_proto::adapter::AdapterEvent;

/// How long the session must stay non-busy before the marker is written.
/// A session pausing between tool calls / API turns flickers idle for a poll
/// or two; 60s is comfortably past that while still ending the pod promptly.
/// Overridable via `CCTUI_DISPATCH_DONE_SETTLE_SECS`.
const DEFAULT_SETTLE_SECS: u64 = 60;

/// Resolve the settle window from `CCTUI_DISPATCH_DONE_SETTLE_SECS`
/// (unset/unparsable ⇒ [`DEFAULT_SETTLE_SECS`]).
pub(super) fn settle_from_env(var: Option<&str>) -> Duration {
    Duration::from_secs(
        var.and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(DEFAULT_SETTLE_SECS),
    )
}

/// Whether `path` holds a JSON verdict, the entrypoint's `result_valid`.
pub(super) fn result_written(path: &Path) -> bool {
    std::fs::read(path)
        .is_ok_and(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).is_ok())
}

pub(super) fn is_reply(event: &AdapterEvent) -> bool {
    match event {
        AdapterEvent::Message { payload, .. } => payload["role"] == "assistant",
        AdapterEvent::ToolUse { .. } => true,
        _ => false,
    }
}

#[derive(Debug)]
pub(super) struct DispatchDoneTracker {
    session_id: String,
    short: String,
    marker_path: PathBuf,
    settle: Duration,
    seen_busy: bool,
    idle_since: Option<Instant>,
    done: bool,
    last_transcript_offset: Option<u64>,
}

impl DispatchDoneTracker {
    pub fn new(session_id: &str, jobs_root: &Path, settle: Duration) -> Self {
        // Same derivation as the spawn path: `short = session_id[..8]`
        // (`chars` so a malformed short id can't panic on a byte boundary).
        let short: String = session_id.chars().take(8).collect();
        let marker_path = jobs_root.join(&short).join("dispatch_done");
        Self {
            session_id: session_id.to_owned(),
            short,
            marker_path,
            settle,
            seen_busy: false,
            idle_since: None,
            done: false,
            last_transcript_offset: None,
        }
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn short(&self) -> &str {
        &self.short
    }

    pub fn marker_path(&self) -> &Path {
        &self.marker_path
    }

    pub const fn seen_busy(&self) -> bool {
        self.seen_busy
    }

    /// Whether the marker has already fired (diagnose observability).
    pub const fn is_done(&self) -> bool {
        self.done
    }

    /// Whether the session finished its work: it settled idle after working,
    /// or wrote its verdict to `RESULT_FILE` after it started working.
    pub const fn finished(&self, result_written: bool) -> bool {
        self.done || (self.seen_busy && result_written)
    }

    /// Whether a live snapshot's `tempo`/`state` counts as busy. `blocked`
    /// (pending prompt) is treated as busy: the turn isn't complete, and the
    /// crash backstops still bound a wedged prompt.
    pub fn is_busy(tempo: Option<&str>, state: Option<&str>) -> bool {
        matches!(tempo, Some("active" | "blocked")) || matches!(state, Some("working" | "running"))
    }

    /// Whether the tracked session's transcript grew since the last poll. A
    /// growing transcript means the session is actively working even if the
    /// control-socket snapshot flickered idle — the exact blind spot that let a
    /// worktree-entered session be reaped mid-work. Callers OR this into `busy`.
    pub fn transcript_grew(&mut self, offset: u64) -> bool {
        let grew = self.last_transcript_offset.is_some_and(|prev| offset > prev);
        self.last_transcript_offset = Some(offset);
        grew
    }

    /// A reply counts as work: a fast answer can go idle between two polls.
    pub const fn note_reply(&mut self) {
        self.seen_busy = true;
    }

    /// Feed one poll observation. Returns `true` exactly once: when the
    /// session has been busy at least once (so a slow cold start is never
    /// mistaken for completion) and has then stayed non-busy for the full
    /// settle window.
    pub fn observe(&mut self, busy: bool, now: Instant) -> bool {
        if self.done {
            return false;
        }
        if busy {
            self.seen_busy = true;
            self.idle_since = None;
            return false;
        }
        if !self.seen_busy {
            return false;
        }
        let since = *self.idle_since.get_or_insert(now);
        if now.duration_since(since) >= self.settle {
            self.done = true;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracker(settle_secs: u64) -> DispatchDoneTracker {
        DispatchDoneTracker::new(
            "abcd1234-5678-90ab-cdef-000000000000",
            Path::new("/tmp/jobs"),
            Duration::from_secs(settle_secs),
        )
    }

    #[test]
    fn short_and_marker_path_derive_from_session_id() {
        let t = tracker(60);
        assert_eq!(t.short(), "abcd1234");
        assert_eq!(t.marker_path(), Path::new("/tmp/jobs/abcd1234/dispatch_done"));
    }

    #[test]
    fn idle_before_any_busy_never_fires() {
        // A slow cold start (never seen busy) must not be read as done.
        let mut t = tracker(60);
        let start = Instant::now();
        assert!(!t.observe(false, start));
        assert!(!t.observe(false, start + Duration::from_hours(1)));
        assert!(!t.seen_busy());
    }

    #[test]
    fn fires_once_after_busy_then_settled_idle() {
        let mut t = tracker(60);
        let start = Instant::now();
        assert!(!t.observe(true, start));
        assert!(t.seen_busy());
        assert!(!t.observe(false, start + Duration::from_secs(10)), "idle < settle");
        assert!(!t.observe(false, start + Duration::from_secs(69)), "59s idle < settle");
        assert!(t.observe(false, start + Duration::from_secs(71)), "61s idle >= settle");
        assert!(!t.observe(false, start + Duration::from_secs(200)), "fires exactly once");
    }

    #[test]
    fn busy_flicker_resets_the_settle_clock() {
        // A pause between tool calls (idle for less than the settle window,
        // then busy again) must not fire.
        let mut t = tracker(60);
        let start = Instant::now();
        assert!(!t.observe(true, start));
        assert!(!t.observe(false, start + Duration::from_secs(30)));
        assert!(!t.observe(true, start + Duration::from_secs(50)), "back to work");
        assert!(
            !t.observe(false, start + Duration::from_secs(70)),
            "clock restarted at the second idle"
        );
        assert!(t.observe(false, start + Duration::from_secs(131)));
    }

    #[test]
    fn a_reply_between_polls_counts_as_work() {
        let mut t = tracker(60);
        let start = Instant::now();
        assert!(!t.observe(false, start), "idle before the answer landed");
        t.note_reply();
        assert!(t.seen_busy());
        assert!(!t.observe(false, start + Duration::from_secs(2)), "idle < settle");
        assert!(t.observe(false, start + Duration::from_secs(63)), "settled idle after the reply");
        assert!(t.finished(false));
    }

    #[test]
    fn a_reply_does_not_end_a_session_still_working_or_blocked() {
        let mut t = tracker(60);
        let start = Instant::now();
        t.note_reply();
        for (secs, tempo, state) in
            [(100, "active", "working"), (200, "blocked", "done"), (300, "idle", "running")]
        {
            let busy = DispatchDoneTracker::is_busy(Some(tempo), Some(state));
            assert!(!t.observe(busy, start + Duration::from_secs(secs)));
        }
        assert!(!t.finished(false), "the settle clock never ran");
        assert!(!t.observe(false, start + Duration::from_secs(301)));
        assert!(t.observe(false, start + Duration::from_secs(361)));
    }

    #[test]
    fn reply_classification() {
        let msg = |role: &str| AdapterEvent::Message {
            local_id: "s".into(),
            payload: serde_json::json!({ "role": role, "text": "391" }),
            turn_id: None,
        };
        assert!(is_reply(&msg("assistant")));
        assert!(is_reply(&AdapterEvent::ToolUse {
            local_id: "s".into(),
            payload: serde_json::json!({}),
        }));
        assert!(!is_reply(&msg("user")), "the dispatched prompt is not work");
        assert!(!is_reply(&msg("system_marker")));
    }

    #[test]
    fn transcript_growth_reports_only_on_increase() {
        let mut t = tracker(60);
        assert!(!t.transcript_grew(0), "first observation sets the baseline, no growth");
        assert!(!t.transcript_grew(0), "unchanged offset is not growth");
        assert!(t.transcript_grew(128), "offset advanced");
        assert!(!t.transcript_grew(128), "held after advancing");
        assert!(t.transcript_grew(256), "advanced again");
    }

    #[test]
    fn transcript_growth_keeps_a_snapshot_idle_session_alive() {
        // The blind-watcher reap: the control snapshot reads idle for the whole
        // window, but the transcript keeps growing (the session works in a
        // worktree). ORing growth into busy must hold the marker off.
        let mut t = tracker(60);
        let start = Instant::now();
        let mut off = 0u64;
        for i in 0..20 {
            off += 64;
            let grew = t.transcript_grew(off);
            let busy = grew || DispatchDoneTracker::is_busy(Some("idle"), Some("done"));
            assert!(
                !t.observe(busy, start + Duration::from_secs(i * 10)),
                "a growing transcript must never fire the marker"
            );
        }
        // Once the transcript stops growing and the snapshot stays idle, the
        // settle clock runs and the marker eventually fires.
        assert!(!t.transcript_grew(off));
        assert!(!t.observe(false, start + Duration::from_secs(210)));
        assert!(t.observe(false, start + Duration::from_secs(271)));
    }

    #[test]
    fn finished_needs_settled_idle_or_a_verdict_after_work() {
        let mut t = tracker(60);
        let start = Instant::now();
        assert_eq!(t.session_id(), "abcd1234-5678-90ab-cdef-000000000000");
        assert!(!t.finished(false));
        assert!(!t.finished(true), "a verdict before any work is a failed start, not a finish");

        assert!(!t.observe(true, start));
        assert!(!t.finished(false), "working, no verdict yet");
        assert!(t.finished(true), "wrote its verdict after working");

        assert!(!t.observe(false, start + Duration::from_secs(1)));
        assert!(!t.finished(false), "idle, not yet settled");
        assert!(t.observe(false, start + Duration::from_secs(62)));
        assert!(t.finished(false), "settled idle after work");
    }

    #[test]
    fn result_written_requires_a_json_verdict() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("result.json");
        assert!(!result_written(&path), "missing");
        std::fs::write(&path, b"").unwrap();
        assert!(!result_written(&path), "empty");
        std::fs::write(&path, b"{\"status\":").unwrap();
        assert!(!result_written(&path), "truncated");
        std::fs::write(&path, br#"{"task_id":"t","status":"success"}"#).unwrap();
        assert!(result_written(&path));
    }

    #[test]
    fn busy_classification() {
        assert!(DispatchDoneTracker::is_busy(Some("active"), None));
        assert!(DispatchDoneTracker::is_busy(Some("blocked"), Some("done")));
        assert!(DispatchDoneTracker::is_busy(Some("idle"), Some("working")));
        assert!(DispatchDoneTracker::is_busy(None, Some("running")));
        assert!(!DispatchDoneTracker::is_busy(Some("idle"), Some("done")));
        assert!(!DispatchDoneTracker::is_busy(None, None));
        assert!(!DispatchDoneTracker::is_busy(Some("hibernated"), Some("failed")));
    }

    #[test]
    fn settle_env_parsing() {
        assert_eq!(settle_from_env(None), Duration::from_mins(1));
        assert_eq!(settle_from_env(Some("15")), Duration::from_secs(15));
        assert_eq!(settle_from_env(Some(" 90 ")), Duration::from_secs(90));
        assert_eq!(settle_from_env(Some("nope")), Duration::from_mins(1));
    }
}
