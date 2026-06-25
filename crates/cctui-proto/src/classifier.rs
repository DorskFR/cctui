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
use std::sync::{Arc, RwLock};
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
    /// Set to `Some(reason)` when the session is currently refused by the
    /// per-account gateway soft limit (CCT-444/CCT-488). This is a durable,
    /// server-owned block (persisted on the session row), independent of the
    /// churning daemon `tempo`/`state` signals — so it must win over a
    /// `busy`/`active` reading from a worker still retrying behind the 429.
    /// When set the session is unconditionally [`Bucket::Blocked`] (needs the
    /// human to continue on another account); `None` means no soft-limit block.
    pub soft_limit_blocked: Option<&'a str>,
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
/// This is the **seam** between the optional GitHub connector and the
/// classifier (docs/github-integration.md §6.1, GH-CLS-1). The cache lives in
/// core (`AppState`) and the classifier reads it; the GitHub connector, when
/// compiled in, *pushes* enriched check/review state into it. Core/classifier
/// never depend on `cctui-github` — the dependency is strictly one-directional
/// (docs §7.5). When GitHub is absent the cache simply stays empty: sessions
/// still render and `SessionChild` links remain opaque core metadata, so the
/// `Review` bucket never arises spuriously and behaviour is byte-for-byte the
/// feature-off baseline.
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
    // A gateway soft-limit block (CCT-488) is a hard, durable "needs input"
    // signal: the worker is locked out of the account until a human switches
    // it. It must win over every transient liveness reading (a worker still
    // hammering Retry-After looks `busy`/`active`), so it is checked first.
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
    fn soft_limit_block_trumps_busy_and_active() {
        // CCT-488: a gateway soft-limit 429 must surface as Blocked even while
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
        // The actual bug: a soft-limited idle session that would otherwise fall
        // through to Working/Done is now durably Blocked (needs input).
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
    fn pr_with_failed_check_is_review() {
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
        // The seam GH-CLS-1 builds on: the connector upserts owned status into
        // the shared cache; the classifier borrows a snapshot of it.
        let children = [SessionChild {
            id: "1".into(),
            href: "https://github.com/o/r/pull/7".into(),
            kind: "pr".into(),
        }];
        let mut s = snap();
        s.state = Some("working");
        s.children = &children;

        let cache = PrStatusCache::new();

        // Degradation: GitHub absent / nothing published yet → empty snapshot,
        // no Review bucket, behaviour identical to the feature-off baseline.
        let snap0 = cache.snapshot();
        assert!(snap0.is_empty());
        assert_eq!(classify(&s, &PrStatusCache::borrow_map(&snap0)), Bucket::Working);

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
        assert_eq!(classify(&s, &PrStatusCache::borrow_map(&snap1)), Bucket::Review);

        // Connector drops the entry (e.g. PR merged & pruned) → back to baseline.
        cache.remove("https://github.com/o/r/pull/7");
        let snap2 = cache.snapshot();
        assert_eq!(classify(&s, &PrStatusCache::borrow_map(&snap2)), Bucket::Working);
    }

    #[test]
    fn bucket_labels() {
        assert_eq!(Bucket::Working.label(), "Working");
        assert_eq!(Bucket::Blocked.label(), "Needs input");
        assert_eq!(Bucket::Review.label(), "Ready for review");
        assert_eq!(Bucket::Done.label(), "Completed");
    }
}
