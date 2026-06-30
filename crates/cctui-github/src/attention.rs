//! GH-CONN-6: per-PR attention-bucket derivation
//! (docs/github-integration.md §6.1).
//!
//! Pure logic mapping a synced PR's state — plus its CI checks, submitted
//! reviews, and the viewer's relationship to the PR (did they author it? are
//! they review-requested?) — onto a single [`AttentionBucket`]. The `/github`
//! inbox (GH-UI-1) groups tracked PRs by the result; the classifier feed
//! (GH-CLS-1) stays the *session*-side enrichment, this is the *PR-inbox* side.
//!
//! No database is required: callers project the CONN-3 row types
//! ([`PullUpsert`]/[`CheckUpsert`]/[`ReviewUpsert`]) — or the equivalent column
//! tuples read back from `github.*` — and call [`derive_bucket`]. The bucket
//! mirrors the session classifier's vocabulary
//! ([`cctui_proto::classifier::Bucket`]) so the two views read the same way.

use cctui_proto::github::{AttentionBucket, CheckUpsert, PullUpsert, ReviewUpsert};

/// CI conclusions that count as a red build — kept in lockstep with
/// [`crate::classifier_feed`] so the inbox and the session classifier agree on
/// what "CI red" means.
fn is_failing_conclusion(conclusion: Option<&str>) -> bool {
    matches!(
        conclusion,
        Some("failure" | "timed_out" | "cancelled" | "action_required" | "startup_failure")
    )
}

/// The viewer's relationship to a PR — the half of the signal that doesn't come
/// from the PR row itself but from who is asking.
#[derive(Debug, Clone, Copy, Default)]
pub struct Viewer {
    /// The viewer authored the PR (`pull.author == viewer login`). Drives the
    /// `MyPr*` buckets.
    pub authored: bool,
    /// The viewer (directly or via a team) is a requested reviewer who hasn't
    /// submitted a review yet. Drives `NeedsMyReview`.
    pub review_requested: bool,
}

/// Derive the single most-actionable [`AttentionBucket`] for a PR.
///
/// Mirrors [`crate::classifier_feed::derive_status`] in its inputs (the CONN-3
/// row types) but answers the inbox's question — "what do I do about this PR?"
/// — rather than the session classifier's coarse `state`.
///
/// Rules (docs §6.1), in priority order:
///
/// 1. **Inactive** PRs (closed or merged) are always [`AttentionBucket::Waiting`]
///    — nothing to act on regardless of CI/reviews.
/// 2. **The viewer's own PR** (`viewer.authored`):
///    - any submitted `changes_requested` review →
///      [`AttentionBucket::MyPrChangesRequested`];
///    - else any failing CI check → [`AttentionBucket::MyPrCiRed`];
///    - else (green/empty CI, no outstanding change-request) →
///      [`AttentionBucket::MyPrMergeable`] — ready to merge or chase approval.
///    - A draft PR is [`AttentionBucket::Waiting`] (not yet asking for action),
///      unless it already has a `changes_requested` (still the author's ball).
/// 3. **Someone else's PR** where the viewer is review-requested and hasn't
///    reviewed → [`AttentionBucket::NeedsMyReview`].
/// 4. **Everything else** → [`AttentionBucket::Waiting`].
#[must_use]
pub fn derive_bucket(
    pull: &PullUpsert,
    checks: &[CheckUpsert],
    reviews: &[ReviewUpsert],
    viewer: Viewer,
) -> AttentionBucket {
    let check_rows: Vec<(String, Option<String>)> =
        checks.iter().map(|c| (c.status.clone(), c.conclusion.clone())).collect();
    let review_rows: Vec<(String,)> = reviews.iter().map(|r| (r.state.clone(),)).collect();
    derive_bucket_from_rows(&pull.state, pull.merged, pull.draft, &check_rows, &review_rows, viewer)
}

/// Pure core of [`derive_bucket`], over already-projected column tuples.
///
/// Kept this way so it is
/// trivially unit-testable and so the DB read path (`SELECT state, merged,
/// draft …` / check `(status, conclusion)` / review `(state,)`) and the struct
/// path agree by construction. Mirrors
/// [`crate::classifier_feed::derive_status_from_rows`]'s projection shape.
#[must_use]
pub fn derive_bucket_from_rows(
    state: &str,
    merged: bool,
    draft: bool,
    checks: &[(String, Option<String>)],
    reviews: &[(String,)],
    viewer: Viewer,
) -> AttentionBucket {
    // 1. Inactive PRs need no attention.
    if merged || !state.eq_ignore_ascii_case("open") {
        return AttentionBucket::Waiting;
    }

    let any_changes_requested =
        reviews.iter().any(|(s,)| s.eq_ignore_ascii_case("changes_requested"));

    // 2. The viewer's own PR.
    if viewer.authored {
        if any_changes_requested {
            return AttentionBucket::MyPrChangesRequested;
        }
        // A draft that isn't already carrying a change-request isn't asking for
        // action yet.
        if draft {
            return AttentionBucket::Waiting;
        }
        let ci_red = checks.iter().any(|(status, conclusion)| {
            status == "completed" && is_failing_conclusion(conclusion.as_deref())
        });
        if ci_red {
            return AttentionBucket::MyPrCiRed;
        }
        return AttentionBucket::MyPrMergeable;
    }

    // 3. Someone else's PR the viewer owes a review on.
    if viewer.review_requested {
        return AttentionBucket::NeedsMyReview;
    }

    // 4. Nothing actionable.
    AttentionBucket::Waiting
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pull(state: &str, merged: bool, draft: bool) -> PullUpsert {
        PullUpsert {
            node_id: "n".into(),
            repo: "o/r".into(),
            number: 7,
            title: "t".into(),
            state: state.into(),
            merged,
            draft,
            mergeable_state: None,
            author: "me".into(),
            head_sha: "deadbeef".into(),
            base_ref: "main".into(),
            head_ref: "feat".into(),
            gh_created_at: "2026-01-01T00:00:00Z".into(),
            gh_updated_at: "2026-01-01T00:00:00Z".into(),
        }
    }

    fn check(status: &str, conclusion: Option<&str>) -> CheckUpsert {
        CheckUpsert {
            repo: "o/r".into(),
            head_sha: "deadbeef".into(),
            external_id: conclusion.unwrap_or(status).into(),
            name: "ci".into(),
            status: status.into(),
            conclusion: conclusion.map(Into::into),
            details_url: None,
        }
    }

    fn review(state: &str) -> ReviewUpsert {
        ReviewUpsert {
            repo: "o/r".into(),
            pull_number: 7,
            review_id: 1,
            reviewer: "rev".into(),
            state: state.into(),
            body: None,
            commit_id: None,
            submitted_at: None,
        }
    }

    const AUTHOR: Viewer = Viewer { authored: true, review_requested: false };
    const REVIEWER: Viewer = Viewer { authored: false, review_requested: true };
    const BYSTANDER: Viewer = Viewer { authored: false, review_requested: false };

    /// Table-driven coverage of the §6.1 rules. Each row: (label, pull, checks,
    /// reviews, viewer, expected bucket).
    // Table-driven test: the case tuple and the long case list are inherent to
    // covering every §6.1 rule in one place; splitting would obscure the table.
    #[allow(clippy::too_many_lines, clippy::type_complexity)]
    #[test]
    fn buckets_match_spec() {
        let cases: Vec<(
            &str,
            PullUpsert,
            Vec<CheckUpsert>,
            Vec<ReviewUpsert>,
            Viewer,
            AttentionBucket,
        )> = vec![
            (
                "authored + CI red",
                pull("open", false, false),
                vec![check("completed", Some("success")), check("completed", Some("failure"))],
                vec![],
                AUTHOR,
                AttentionBucket::MyPrCiRed,
            ),
            (
                "authored + changes requested beats CI red",
                pull("open", false, false),
                vec![check("completed", Some("failure"))],
                vec![review("changes_requested")],
                AUTHOR,
                AttentionBucket::MyPrChangesRequested,
            ),
            (
                "authored + green CI + approved → mergeable",
                pull("open", false, false),
                vec![check("completed", Some("success"))],
                vec![review("approved")],
                AUTHOR,
                AttentionBucket::MyPrMergeable,
            ),
            (
                "authored + no checks, no reviews → mergeable",
                pull("open", false, false),
                vec![],
                vec![],
                AUTHOR,
                AttentionBucket::MyPrMergeable,
            ),
            (
                "authored + running CI (not completed) → mergeable, not CI red",
                pull("open", false, false),
                vec![check("in_progress", None)],
                vec![],
                AUTHOR,
                AttentionBucket::MyPrMergeable,
            ),
            (
                "authored draft → waiting",
                pull("open", false, true),
                vec![check("completed", Some("failure"))],
                vec![],
                AUTHOR,
                AttentionBucket::Waiting,
            ),
            (
                "authored draft WITH changes requested → still author's ball",
                pull("open", false, true),
                vec![],
                vec![review("changes_requested")],
                AUTHOR,
                AttentionBucket::MyPrChangesRequested,
            ),
            (
                "review-requested on someone else's PR → needs my review",
                pull("open", false, false),
                vec![check("completed", Some("success"))],
                vec![],
                REVIEWER,
                AttentionBucket::NeedsMyReview,
            ),
            (
                "bystander on an open PR → waiting",
                pull("open", false, false),
                vec![],
                vec![],
                BYSTANDER,
                AttentionBucket::Waiting,
            ),
            (
                "authored but merged → waiting",
                pull("closed", true, false),
                vec![check("completed", Some("failure"))],
                vec![review("changes_requested")],
                AUTHOR,
                AttentionBucket::Waiting,
            ),
            (
                "authored but closed → waiting",
                pull("closed", false, false),
                vec![],
                vec![review("changes_requested")],
                AUTHOR,
                AttentionBucket::Waiting,
            ),
            (
                "review-requested but PR already merged → waiting",
                pull("closed", true, false),
                vec![],
                vec![],
                REVIEWER,
                AttentionBucket::Waiting,
            ),
            (
                "author takes precedence over a stale review-request flag",
                pull("open", false, false),
                vec![check("completed", Some("timed_out"))],
                vec![],
                Viewer { authored: true, review_requested: true },
                AttentionBucket::MyPrCiRed,
            ),
        ];

        for (label, p, checks, reviews, viewer, expected) in cases {
            let got = derive_bucket(&p, &checks, &reviews, viewer);
            assert_eq!(got, expected, "case: {label}");
        }
    }

    #[test]
    fn struct_and_row_paths_agree() {
        let p = pull("open", false, false);
        let checks = [check("completed", Some("failure"))];
        let reviews: [ReviewUpsert; 0] = [];
        let via_struct = derive_bucket(&p, &checks, &reviews, AUTHOR);
        let via_rows = derive_bucket_from_rows(
            "open",
            false,
            false,
            &[("completed".into(), Some("failure".into()))],
            &[],
            AUTHOR,
        );
        assert_eq!(via_struct, via_rows);
        assert_eq!(via_struct, AttentionBucket::MyPrCiRed);
    }

    #[test]
    fn each_failing_conclusion_is_ci_red() {
        for c in ["failure", "timed_out", "cancelled", "action_required", "startup_failure"] {
            let got = derive_bucket(
                &pull("open", false, false),
                &[check("completed", Some(c))],
                &[],
                AUTHOR,
            );
            assert_eq!(got, AttentionBucket::MyPrCiRed, "conclusion {c}");
        }
        // A benign conclusion is not red.
        for c in ["success", "neutral", "skipped"] {
            let got = derive_bucket(
                &pull("open", false, false),
                &[check("completed", Some(c))],
                &[],
                AUTHOR,
            );
            assert_eq!(got, AttentionBucket::MyPrMergeable, "conclusion {c}");
        }
    }
}
