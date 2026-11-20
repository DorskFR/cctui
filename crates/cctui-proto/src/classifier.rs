//! Port of the `claude agents` TUI's session classifier.
//!
//! Groups sessions into three buckets: Working / Needs input / Completed.
//!
//! Two callers: the cctui TUI and the web UI. Both consume the same
//! [`AdapterEvent::Status`] fields; this module is the single source of
//! truth for how they map onto the buckets.

use crate::adapter::SessionChild;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
#[cfg(feature = "ts")]
use ts_rs::TS;

/// The session buckets clients group on.
///
/// Serialized `snake_case` (`working` / `blocked` / `done`) and shared verbatim
/// by the TUI and web UI as the on-wire grouping signal. `review` is what older
/// servers sent for a session whose PR needs attention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum Bucket {
    Working,
    #[serde(alias = "review")]
    Blocked,
    Done,
}

impl Bucket {
    /// Stable label suitable for UI rendering.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Working => "Working",
            Self::Blocked => "Needs input",
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
    /// Set to `Some(reason)` when the session is currently refused by the
    /// per-account gateway soft limit. This is a durable,
    /// server-owned block (persisted on the session row), independent of the
    /// churning daemon `tempo`/`state` signals — so it must win over a
    /// `busy`/`active` reading from a worker still retrying behind the 429.
    /// When set the session is unconditionally [`Bucket::Blocked`] (needs the
    /// human to continue on another account); `None` means no soft-limit block.
    pub soft_limit_blocked: Option<&'a str>,
    /// The row is `ended` or `failed`: Completed whatever signal it kept.
    pub ended: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct PrStatus<'a> {
    pub state: &'a str,
    pub checks_passed: u32,
    pub checks_failed: u32,
    pub checks_pending: u32,
    pub review: &'a str,
}

/// Owned counterpart of [`PrStatus`], suitable for storing in the shared
/// best-effort PR status cache ([`PrStatusCache`]).
///
/// [`PrStatus`] borrows its strings so the classifier hot path allocates
/// nothing, but the cache must own its entries (the connector that wrote them
/// has long since returned). Call [`OwnedPrStatus::as_ref`] to obtain a borrowed
/// [`PrStatus`] for [`classify`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedPrStatus {
    pub state: String,
    pub checks_passed: u32,
    pub checks_failed: u32,
    pub checks_pending: u32,
    pub review: String,
}

impl OwnedPrStatus {
    /// Borrow this owned status as a [`PrStatus`] for classification.
    #[must_use]
    pub fn as_ref(&self) -> PrStatus<'_> {
        PrStatus {
            state: &self.state,
            checks_passed: self.checks_passed,
            checks_failed: self.checks_failed,
            checks_pending: self.checks_pending,
            review: &self.review,
        }
    }
}

/// Shared, core-owned, best-effort PR status cache keyed by `SessionChild.href`.
///
/// The GitHub connector pushes check/review state in; the classifier reads it.
/// Core never depends on `cctui-github`. Without GitHub the cache stays empty
/// and no session is blocked on its PR.
#[derive(Debug, Clone, Default)]
pub struct PrStatusCache {
    inner: Arc<RwLock<HashMap<String, OwnedPrStatus>>>,
}

impl PrStatusCache {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace the cached status for `href` (a `SessionChild.href`).
    /// Called by the GitHub connector after an upsert.
    pub fn upsert(&self, href: impl Into<String>, status: OwnedPrStatus) {
        if let Ok(mut map) = self.inner.write() {
            map.insert(href.into(), status);
        }
    }

    /// Drop the cached status for `href`, if any.
    pub fn remove(&self, href: &str) {
        if let Ok(mut map) = self.inner.write() {
            map.remove(href);
        }
    }

    /// Take an owned snapshot of the cache for a classification pass.
    ///
    /// [`classify`] borrows [`PrStatus`] out of a `HashMap`, so the caller holds
    /// this snapshot for the duration of the pass and builds the borrowed map
    /// via [`Self::borrow_map`]. A poisoned lock degrades to an empty snapshot
    /// rather than panicking — losing enrichment, never blocking the list.
    #[must_use]
    pub fn snapshot(&self) -> HashMap<String, OwnedPrStatus> {
        self.inner.read().map(|m| m.clone()).unwrap_or_default()
    }

    /// Build the borrowed `PrStatus` map that [`classify`] expects from an owned
    /// snapshot (see [`Self::snapshot`]).
    #[must_use]
    pub fn borrow_map(snapshot: &HashMap<String, OwnedPrStatus>) -> HashMap<String, PrStatus<'_>> {
        snapshot.iter().map(|(k, v)| (k.clone(), v.as_ref())).collect()
    }
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

/// Whether the agent itself reported the turn over: a terminal state or an
/// activity verdict. A row that merely stopped reporting does not count.
#[must_use]
pub fn reports_finished(state: Option<&str>, activity: Option<&str>) -> bool {
    is_terminal(state) || matches!(activity, Some("failure" | "stopped" | "success"))
}

/// Apply the classifier. First match wins:
///
/// ```text
/// ended row                                → Done
/// soft-limit block                         → Blocked
/// q == busy                                → Working
/// activity in {failure, stopped}           → Done
/// q == waiting                             → Blocked
/// idle with a PR needing attention         → Blocked
/// activity == success                      → Done
/// tempo == blocked                         → Blocked
/// tempo == active, state working/running,
///   or no signal at all                    → Working
/// otherwise                                → Done
/// ```
#[must_use]
pub fn classify(
    input: &ClassifyInput<'_>,
    pr_cache: &HashMap<String, PrStatus<'_>, impl std::hash::BuildHasher>,
) -> Bucket {
    if input.ended {
        return Bucket::Done;
    }
    // A worker still hammering Retry-After behind a soft-limit 429 looks
    // `busy`/`active`, so the block is checked before any liveness reading.
    if input.soft_limit_blocked.is_some() {
        return Bucket::Blocked;
    }
    if input.q == Some("busy") {
        return Bucket::Working;
    }
    if input.activity == Some("failure") || input.activity == Some("stopped") {
        return Bucket::Done;
    }
    if input.q == Some("waiting") {
        return Bucket::Blocked;
    }
    let working = input.tempo == Some("active")
        || (matches!(input.state, Some("working" | "running"))
            && input.tempo != Some("hibernated"));
    if !working && !is_terminal(input.state) && has_pr_attention(input.children, pr_cache) {
        return Bucket::Blocked;
    }
    if input.activity == Some("success") {
        return Bucket::Done;
    }
    if input.tempo == Some("blocked") {
        return Bucket::Blocked;
    }
    let unreported = input.tempo.is_none() && input.state.is_none() && input.activity.is_none();
    if working || unreported {
        return Bucket::Working;
    }
    Bucket::Done
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
    fn soft_limit_block_trumps_busy_and_active() {
        // a gateway soft-limit 429 must surface as Blocked even while
        // the worker still looks busy/active retrying behind the Retry-After.
        let mut s = snap();
        s.q = Some("busy");
        s.tempo = Some("active");
        s.state = Some("working");
        s.soft_limit_blocked = Some("switch account: foo rate-limited");
        assert_eq!(classify(&s, &HashMap::new()), Bucket::Blocked);
    }

    #[test]
    fn soft_limit_block_trumps_idle_done() {
        // A soft-limited idle session is Blocked, not Working/Done.
        let mut s = snap();
        s.activity = Some("success");
        s.soft_limit_blocked = Some("switch account: foo rate-limited");
        assert_eq!(classify(&s, &HashMap::new()), Bucket::Blocked);
    }

    #[test]
    fn stopped_state_is_done() {
        let mut s = snap();
        s.state = Some("stopped");
        assert_eq!(classify(&s, &HashMap::new()), Bucket::Done);
    }

    #[test]
    fn an_ended_row_is_done_whatever_signal_it_kept() {
        let mut s = snap();
        s.ended = true;
        s.tempo = Some("active");
        s.state = Some("running");
        assert_eq!(classify(&s, &HashMap::new()), Bucket::Done);
        s.q = Some("busy");
        s.soft_limit_blocked = Some("switch account");
        assert_eq!(classify(&s, &HashMap::new()), Bucket::Done);
        s.tempo = Some("blocked");
        assert_eq!(classify(&s, &HashMap::new()), Bucket::Done);
    }

    #[test]
    fn an_idle_row_without_a_terminal_state_is_done() {
        let mut s = snap();
        s.tempo = Some("idle");
        s.state = Some("idle");
        assert_eq!(classify(&s, &HashMap::new()), Bucket::Done);
        s.state = Some("adopted");
        assert_eq!(classify(&s, &HashMap::new()), Bucket::Done);
    }

    #[test]
    fn a_row_that_never_reported_is_working() {
        assert_eq!(classify(&snap(), &HashMap::new()), Bucket::Working);
    }

    #[test]
    fn an_idle_agent_still_on_a_background_task_is_working() {
        let mut s = snap();
        s.tempo = Some("idle");
        s.state = Some("working");
        assert_eq!(classify(&s, &HashMap::new()), Bucket::Working);
    }

    #[test]
    fn a_hibernated_worker_that_died_mid_turn_is_not_working() {
        let mut s = snap();
        s.tempo = Some("hibernated");
        s.state = Some("working");
        assert_eq!(classify(&s, &HashMap::new()), Bucket::Done);
        s.state = Some("stopped");
        assert_eq!(classify(&s, &HashMap::new()), Bucket::Done);
    }

    #[test]
    fn reports_finished_needs_a_terminal_state_or_a_verdict() {
        assert!(reports_finished(Some("done"), None));
        assert!(reports_finished(None, Some("success")));
        assert!(!reports_finished(Some("idle"), None));
        assert!(!reports_finished(None, None));
    }

    #[test]
    fn the_retired_review_key_reads_as_blocked() {
        assert_eq!(serde_json::from_str::<Bucket>("\"review\"").unwrap(), Bucket::Blocked);
        assert_eq!(serde_json::to_string(&Bucket::Blocked).unwrap(), "\"blocked\"");
    }

    #[test]
    fn an_idle_session_with_a_failed_pr_check_is_blocked() {
        let children = [SessionChild {
            id: "1".into(),
            href: "https://github.com/o/r/pull/1".into(),
            kind: "pr".into(),
        }];
        let mut cache: HashMap<String, PrStatus<'_>> = HashMap::new();
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
        s.tempo = Some("idle");
        s.state = Some("idle");
        s.children = &children;
        assert_eq!(classify(&s, &cache), Bucket::Blocked);
        s.state = Some("working");
        assert_eq!(classify(&s, &cache), Bucket::Working, "a running turn is not waiting");
    }

    #[test]
    fn pr_approved_with_pending_does_not_block() {
        let children = [SessionChild {
            id: "1".into(),
            href: "https://github.com/o/r/pull/1".into(),
            kind: "pr".into(),
        }];
        let mut cache: HashMap<String, PrStatus<'_>> = HashMap::new();
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
        assert_eq!(classify(&s, &cache), Bucket::Done);
    }

    #[test]
    fn merged_pr_does_not_block() {
        let children = [SessionChild {
            id: "1".into(),
            href: "https://github.com/o/r/pull/1".into(),
            kind: "pr".into(),
        }];
        let mut cache: HashMap<String, PrStatus<'_>> = HashMap::new();
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
    fn cache_seam_enriches_then_degrades() {
        // The connector upserts status into the cache; the classifier snapshots it.
        let children = [SessionChild {
            id: "1".into(),
            href: "https://github.com/o/r/pull/7".into(),
            kind: "pr".into(),
        }];
        let mut s = snap();
        s.state = Some("idle");
        s.children = &children;

        let cache = PrStatusCache::new();

        // GitHub absent or nothing published yet: an empty snapshot blocks nothing.
        let snap0 = cache.snapshot();
        assert!(snap0.is_empty());
        assert_eq!(classify(&s, &PrStatusCache::borrow_map(&snap0)), Bucket::Done);

        // Connector publishes a CI-red status for the PR this session opened.
        cache.upsert(
            "https://github.com/o/r/pull/7",
            OwnedPrStatus {
                state: "OPEN".into(),
                checks_passed: 3,
                checks_failed: 1,
                checks_pending: 0,
                review: "REVIEW_REQUIRED".into(),
            },
        );
        let snap1 = cache.snapshot();
        assert_eq!(classify(&s, &PrStatusCache::borrow_map(&snap1)), Bucket::Blocked);

        // Connector drops the entry (e.g. PR merged & pruned) → back to baseline.
        cache.remove("https://github.com/o/r/pull/7");
        let snap2 = cache.snapshot();
        assert_eq!(classify(&s, &PrStatusCache::borrow_map(&snap2)), Bucket::Done);
    }

    #[test]
    fn bucket_labels() {
        assert_eq!(Bucket::Working.label(), "Working");
        assert_eq!(Bucket::Blocked.label(), "Needs input");
        assert_eq!(Bucket::Done.label(), "Completed");
    }
}
