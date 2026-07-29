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

#[derive(Debug)]
pub(super) struct DispatchDoneTracker {
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
            short,
            marker_path,
            settle,
            seen_busy: false,
            idle_since: None,
            done: false,
            last_transcript_offset: None,
        }
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
