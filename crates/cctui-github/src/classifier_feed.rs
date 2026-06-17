//! GH-CLS-1: feed connector PR/check/review state into the core session
//! classifier's PR status cache (docs/github-integration.md §6.1).
//!
//! A session that opened PR #N carries a `SessionChild { kind: "pr", href }`
//! pointing at the PR's GitHub URL. The session classifier
//! ([`cctui_proto::classifier`]) reads a best-effort, core-owned
//! [`PrStatusCache`] keyed by that `href` to flip the session into
//! *Ready for review* / *CI red*. This module is the **push** side of that
//! seam: it maps the connector's synced GitHub state into a
//! [`cctui_proto::classifier::OwnedPrStatus`] and upserts it under the PR's
//! `href`.
//!
//! The dependency stays one-directional: `cctui-github` depends on
//! `cctui-proto` (which owns the cache type), never the reverse. With the
//! `github` feature off this module isn't compiled and the cache stays empty —
//! sessions still render and no `Review` bucket arises (docs §7.5).

use cctui_proto::classifier::{OwnedPrStatus, PrStatusCache};
use cctui_proto::github::{CheckUpsert, PullUpsert, ReviewUpsert};
use sqlx::PgPool;
use uuid::Uuid;

/// The `SessionChild.href` a PR is linked under: its canonical GitHub URL.
///
/// Sessions store this exact form when they open a PR, so the cache key the
/// connector writes must match it byte-for-byte for the classifier lookup to
/// hit.
#[must_use]
pub fn pr_href(repo: &str, number: i64) -> String {
    format!("https://github.com/{repo}/pull/{number}")
}

/// Map the connector's synced view of a PR — its row plus all known checks for
/// the head SHA and all submitted reviews — into the classifier's owned status.
///
/// The classifier compares against `state == "OPEN"`, `review == "APPROVED"`,
/// and `review == "REVIEW_REQUIRED"`, so this is where GitHub's lowercase
/// `open`/`closed` + `merged` flag and per-reviewer review states collapse onto
/// those tokens.
///
/// - **state**: `MERGED` if merged, else `OPEN`/`CLOSED` from `pull.state`.
/// - **checks**: counted across `checks` — `failure`/`timed_out`/`cancelled`/
///   `action_required` → failed; not-yet-`completed` → pending; `success`/
///   `neutral`/`skipped` → passed.
/// - **review**: the strongest signal across reviews — any
///   `changes_requested` → `CHANGES_REQUESTED`; else any `approved` →
///   `APPROVED`; else `REVIEW_REQUIRED`.
#[must_use]
pub fn derive_status(
    pull: &PullUpsert,
    checks: &[CheckUpsert],
    reviews: &[ReviewUpsert],
) -> OwnedPrStatus {
    // Project onto the row-tuple core so the struct and DB paths agree by
    // construction (latest-wins-per-reviewer is GitHub's real semantics, but the
    // coarse signal only needs the strongest outstanding state).
    let check_rows: Vec<(String, Option<String>)> =
        checks.iter().map(|c| (c.status.clone(), c.conclusion.clone())).collect();
    let review_rows: Vec<(String,)> = reviews.iter().map(|r| (r.state.clone(),)).collect();
    derive_status_from_rows(&pull.state, pull.merged, &check_rows, &review_rows)
}

/// Publish a PR's derived status into the shared classifier cache.
///
/// Best-effort: the cache is advisory, so a poisoned lock simply drops the
/// enrichment (handled inside [`PrStatusCache`]) rather than failing the caller.
pub fn publish(
    cache: &PrStatusCache,
    pull: &PullUpsert,
    checks: &[CheckUpsert],
    reviews: &[ReviewUpsert],
) {
    let href = pr_href(&pull.repo, pull.number);
    cache.upsert(href, derive_status(pull, checks, reviews));
}

/// Re-read a PR's synced state from `github.*` and push its derived classifier
/// status into the shared cache.
///
/// Called after a webhook upsert that touched a PR (or its checks/reviews). It
/// reads the `pull` row plus all checks for its head SHA and all submitted
/// reviews, derives the coarse status via [`derive_status`], and publishes it
/// keyed by the PR's `href`. Best-effort: a missing PR row or any query error
/// leaves the cache untouched (the classifier just keeps its prior view) rather
/// than failing the webhook — the cache is advisory (docs §7.5).
pub async fn refresh(
    pool: &PgPool,
    cache: &PrStatusCache,
    connector_id: Uuid,
    repo: &str,
    number: i64,
) {
    let pull: Option<(String, bool, String)> = sqlx::query_as(
        "SELECT state, merged, head_sha FROM github.pulls \
         WHERE connector_id = $1 AND repo = $2 AND number = $3",
    )
    .bind(connector_id)
    .bind(repo)
    .bind(number)
    .fetch_optional(pool)
    .await
    .unwrap_or(None);
    let Some((state, merged, head_sha)) = pull else { return };

    let checks: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT status, conclusion FROM github.checks \
         WHERE connector_id = $1 AND repo = $2 AND head_sha = $3",
    )
    .bind(connector_id)
    .bind(repo)
    .bind(&head_sha)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    let reviews: Vec<(String,)> = sqlx::query_as(
        "SELECT state FROM github.reviews \
         WHERE connector_id = $1 AND repo = $2 AND pull_number = $3",
    )
    .bind(connector_id)
    .bind(repo)
    .bind(number)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    let status = derive_status_from_rows(&state, merged, &checks, &reviews);
    cache.upsert(pr_href(repo, number), status);
}

/// Pure core of [`refresh`], working on already-fetched column tuples so it is
/// unit-testable without a database. Mirrors [`derive_status`] but over the
/// projected `(state, conclusion)` / `(state,)` rows the queries return.
#[must_use]
fn derive_status_from_rows(
    state: &str,
    merged: bool,
    checks: &[(String, Option<String>)],
    reviews: &[(String,)],
) -> OwnedPrStatus {
    let pr_state = if merged { "MERGED".to_string() } else { state.to_ascii_uppercase() };

    let mut passed = 0u32;
    let mut failed = 0u32;
    let mut pending = 0u32;
    for (status, conclusion) in checks {
        if status != "completed" {
            pending += 1;
            continue;
        }
        match conclusion.as_deref() {
            Some("failure" | "timed_out" | "cancelled" | "action_required" | "startup_failure") => {
                failed += 1;
            }
            _ => passed += 1,
        }
    }

    let any_changes = reviews.iter().any(|(s,)| s.eq_ignore_ascii_case("changes_requested"));
    let any_approved = reviews.iter().any(|(s,)| s.eq_ignore_ascii_case("approved"));
    let review = if any_changes {
        "CHANGES_REQUESTED"
    } else if any_approved {
        "APPROVED"
    } else {
        "REVIEW_REQUIRED"
    }
    .to_string();

    OwnedPrStatus {
        state: pr_state,
        checks_passed: passed,
        checks_failed: failed,
        checks_pending: pending,
        review,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cctui_proto::adapter::SessionChild;
    use cctui_proto::classifier::{Bucket, ClassifyInput, classify};

    fn pull(state: &str, merged: bool) -> PullUpsert {
        PullUpsert {
            node_id: "n".into(),
            repo: "o/r".into(),
            number: 7,
            title: "t".into(),
            state: state.into(),
            merged,
            draft: false,
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

    #[test]
    fn href_matches_session_child_form() {
        assert_eq!(pr_href("o/r", 7), "https://github.com/o/r/pull/7");
    }

    #[test]
    fn maps_open_with_failed_check() {
        let s = derive_status(
            &pull("open", false),
            &[check("completed", Some("success")), check("completed", Some("failure"))],
            &[],
        );
        assert_eq!(s.state, "OPEN");
        assert_eq!((s.checks_passed, s.checks_failed, s.checks_pending), (1, 1, 0));
        assert_eq!(s.review, "REVIEW_REQUIRED");
    }

    #[test]
    fn running_check_is_pending_and_merged_state() {
        let s = derive_status(
            &pull("closed", true),
            &[check("in_progress", None)],
            &[review("approved")],
        );
        assert_eq!(s.state, "MERGED");
        assert_eq!(s.checks_pending, 1);
        assert_eq!(s.review, "APPROVED");
    }

    #[test]
    fn changes_requested_dominates_approval() {
        let s = derive_status(
            &pull("open", false),
            &[],
            &[review("approved"), review("changes_requested")],
        );
        assert_eq!(s.review, "CHANGES_REQUESTED");
    }

    /// End-to-end through the real seam: publish into the cache, then run the
    /// core classifier exactly as the server does.
    #[test]
    fn publish_flips_session_to_review() {
        let cache = PrStatusCache::new();
        publish(&cache, &pull("open", false), &[check("completed", Some("failure"))], &[]);

        let children =
            [SessionChild { id: "1".into(), href: pr_href("o/r", 7), kind: "pr".into() }];
        let input =
            ClassifyInput { state: Some("working"), children: &children, ..Default::default() };
        let snap = cache.snapshot();
        assert_eq!(classify(&input, &PrStatusCache::borrow_map(&snap)), Bucket::Review);
    }
}
