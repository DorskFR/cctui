//! Port of the `claude agents` TUI's session classifier.
//!
//! Groups sessions into four buckets matching the upstream TUI:
//! Working / Needs input / Ready for review / Completed.
//!
//! Two callers: the cctui TUI and the web UI. Both consume the same
//! [`AdapterEvent::Status`] fields; this module is the single source of
//! truth for how they map onto the four buckets.

use crate::adapter::SessionChild;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use ts_rs::TS;

/// The four session buckets clients group on. Serialized `snake_case`
/// (`working` / `blocked` / `review` / `done`) and shared verbatim by the
/// TUI and web UI as the on-wire grouping signal (CCT-90).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum Bucket {
    Working,
    Blocked,
    Review,
    Done,
}

impl Bucket {
    /// Stable label suitable for UI rendering.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Working => "Working",
            Self::Blocked => "Needs input",
            Self::Review => "Ready for review",
            Self::Done => "Completed",
        }
    }
}

/// Inputs to the classifier — projection of [`AdapterEvent::Status`]
/// plus the optional PR-attention cache.
#[derive(Debug, Clone, Default)]
pub struct ClassifyInput<'a> {
    pub tempo: Option<&'a str>,
    pub state: Option<&'a str>,
    pub activity: Option<&'a str>,
    pub children: &'a [SessionChild],
    /// Set to `Some("busy" | "waiting")` if the caller tracks transient
    /// user-activity hints in memory (the binary's `q` parameter). `None`
    /// works fine — we just lose the priority-1 short-circuit.
    pub q: Option<&'a str>,
}

#[derive(Debug, Clone, Copy)]
pub struct PrStatus<'a> {
    pub state: &'a str,
    pub checks_passed: u32,
    pub checks_failed: u32,
    pub checks_pending: u32,
    pub review: &'a str,
}

impl PrStatus<'_> {
    fn level(&self) -> &'static str {
        if self.checks_failed > 0 {
            "error"
        } else if self.checks_pending > 0 || self.review == "REVIEW_REQUIRED" {
            "warning"
        } else {
            "ok"
        }
    }
}

#[must_use]
fn is_terminal(state: Option<&str>) -> bool {
    matches!(state, Some("done" | "stopped" | "killed" | "failed"))
}

/// Apply the classifier. Equivalent (with `q: None`) to:
///
/// ```text
/// def classify(snap, pr_cache):
///     if snap.tempo == "active":              return Working
///     if snap.tempo == "blocked":             return Blocked
///     if snap.state in {"stopped","killed"}:  return Done
///     if snap.state in {"working","running"}: return Working
///     if has_pr_attention(snap, pr_cache):    return Review
///     return Done
/// ```
///
/// — but the full binary algorithm honours `q` (transient user activity)
/// and `activity` (persisted user-stop / failure) too.
#[must_use]
pub fn classify(
    input: &ClassifyInput<'_>,
    pr_cache: &HashMap<String, PrStatus<'_>, impl std::hash::BuildHasher>,
) -> Bucket {
    if input.q == Some("busy") {
        return Bucket::Working;
    }
    if input.activity == Some("failure") || input.activity == Some("stopped") {
        return Bucket::Done;
    }
    if input.q == Some("waiting") {
        return Bucket::Blocked;
    }
    if !is_terminal(input.state) && has_pr_attention(input.children, pr_cache) {
        return Bucket::Review;
    }
    if input.activity == Some("success") {
        return Bucket::Done;
    }
    if input.tempo == Some("blocked") {
        return Bucket::Blocked;
    }
    if input.tempo == Some("active") || matches!(input.state, Some("working" | "running")) {
        return Bucket::Working;
    }
    if is_terminal(input.state) {
        return Bucket::Done;
    }
    Bucket::Working
}

fn has_pr_attention(
    children: &[SessionChild],
    pr_cache: &HashMap<String, PrStatus<'_>, impl std::hash::BuildHasher>,
) -> bool {
    children.iter().any(|child| {
        if child.kind != "pr" {
            return false;
        }
        let Some(pr) = pr_cache.get(&child.href) else { return false };
        if pr.state != "OPEN" {
            return false;
        }
        let level = pr.level();
        level == "error" || (level == "warning" && pr.review != "APPROVED")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap() -> ClassifyInput<'static> {
        ClassifyInput::default()
    }

    #[test]
    fn busy_q_short_circuits_to_working() {
        let mut s = snap();
        s.q = Some("busy");
        s.state = Some("done");
        assert_eq!(classify(&s, &HashMap::new()), Bucket::Working);
    }

    #[test]
    fn failure_activity_is_done() {
        let mut s = snap();
        s.activity = Some("failure");
        assert_eq!(classify(&s, &HashMap::new()), Bucket::Done);
    }

    #[test]
    fn tempo_blocked_trumps_state_working() {
        let mut s = snap();
        s.state = Some("working");
        s.tempo = Some("blocked");
        assert_eq!(classify(&s, &HashMap::new()), Bucket::Blocked);
    }

    #[test]
    fn tempo_active_with_state_done_is_working() {
        // The protocol-doc gotcha: tempo trumps state.
        let mut s = snap();
        s.state = Some("done");
        s.tempo = Some("active");
        assert_eq!(classify(&s, &HashMap::new()), Bucket::Working);
    }

    #[test]
    fn stopped_state_is_done() {
        let mut s = snap();
        s.state = Some("stopped");
        assert_eq!(classify(&s, &HashMap::new()), Bucket::Done);
    }

    #[test]
    fn pr_with_failed_check_is_review() {
        let children = [SessionChild {
            id: "1".into(),
            href: "https://github.com/o/r/pull/1".into(),
            kind: "pr".into(),
        }];
        let mut cache = HashMap::new();
        cache.insert(
            "https://github.com/o/r/pull/1".into(),
            PrStatus {
                state: "OPEN",
                checks_passed: 10,
                checks_failed: 1,
                checks_pending: 0,
                review: "APPROVED",
            },
        );
        let mut s = snap();
        s.state = Some("working");
        s.children = &children;
        assert_eq!(classify(&s, &cache), Bucket::Review);
    }

    #[test]
    fn pr_approved_with_pending_is_not_review() {
        let children = [SessionChild {
            id: "1".into(),
            href: "https://github.com/o/r/pull/1".into(),
            kind: "pr".into(),
        }];
        let mut cache = HashMap::new();
        cache.insert(
            "https://github.com/o/r/pull/1".into(),
            PrStatus {
                state: "OPEN",
                checks_passed: 10,
                checks_failed: 0,
                checks_pending: 1,
                review: "APPROVED",
            },
        );
        let mut s = snap();
        s.state = Some("done");
        s.children = &children;
        // Not Review (warning + APPROVED), state=done → falls through to Done.
        assert_eq!(classify(&s, &cache), Bucket::Done);
    }

    #[test]
    fn merged_pr_does_not_trigger_review() {
        let children = [SessionChild {
            id: "1".into(),
            href: "https://github.com/o/r/pull/1".into(),
            kind: "pr".into(),
        }];
        let mut cache = HashMap::new();
        cache.insert(
            "https://github.com/o/r/pull/1".into(),
            PrStatus {
                state: "MERGED",
                checks_passed: 10,
                checks_failed: 1,
                checks_pending: 0,
                review: "APPROVED",
            },
        );
        let mut s = snap();
        s.state = Some("done");
        s.children = &children;
        assert_eq!(classify(&s, &cache), Bucket::Done);
    }

    #[test]
    fn bucket_labels() {
        assert_eq!(Bucket::Working.label(), "Working");
        assert_eq!(Bucket::Blocked.label(), "Needs input");
        assert_eq!(Bucket::Review.label(), "Ready for review");
        assert_eq!(Bucket::Done.label(), "Completed");
    }
}
