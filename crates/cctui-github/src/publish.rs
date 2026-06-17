//! GH-VIEW-5: publish a review draft as ONE batched GitHub review, plus the
//! pull-down of existing open GitHub review threads.
//!
//! The publish flow (docs/github-integration.md §6.2):
//! 1. Load the open draft (GH-VIEW-4) + its inline comments.
//! 2. Fetch the PR's current diff (GH-VIEW-1) so each draft comment can be
//!    re-anchored against the **current head SHA** via GH-VIEW-2
//!    [`crate::anchor::resolve`].
//! 3. A comment made against a **stale head SHA** (the PR was force-pushed since
//!    the draft was authored) refuses the whole publish with a clear error
//!    ([`PublishError::StaleHeadSha`]) rather than mis-placing comments onto the
//!    wrong lines. A comment whose line simply vanished from the diff is *skipped*
//!    and reported (not fatal) so the rest of the review still posts.
//! 4. The anchored comments + verdict are submitted as ONE
//!    `POST /repos/{o}/{r}/pulls/{n}/reviews` — no per-comment spam.
//! 5. On success the draft is marked `status = published` and each posted
//!    comment's returned GitHub id is stored back on its draft-comment row.
//!
//! The GitHub HTTP surface is behind the [`ReviewSubmitClient`] trait so payload
//! assembly and the stale-SHA refusal are unit-testable without a network (the
//! reqwest-backed [`HttpReviewClient`] is the production impl).
//!
//! Secrets: the connector credential is decrypted only to build the
//! `Authorization` header and is **never** logged; review payloads carry no
//! secret material.

use cctui_proto::github::{
    AnchorError, DraftCommentInfo, PullDiff, ReviewDraftInfo, ReviewVerdict, SkippedComment,
};

const API_BASE: &str = "https://api.github.com";
const USER_AGENT: &str = concat!("cctui/", env!("CARGO_PKG_VERSION"));

/// One comment in a batched review submission, already resolved to a GitHub
/// anchor. Mirrors the per-comment shape of `POST .../reviews`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitComment {
    /// Source draft-comment id, so the returned GitHub id can be written back.
    pub draft_comment_id: uuid::Uuid,
    pub path: String,
    /// GitHub's review-comment `side` token (`LEFT`/`RIGHT`).
    pub side: String,
    /// The (1-based) line on `side` (END line for a multi-line range).
    pub line: u32,
    /// START line of a multi-line range (`None` for a single line).
    pub start_line: Option<u32>,
    /// START side, always equal to `side` here.
    pub start_side: Option<String>,
    pub body: String,
}

/// A fully assembled batched-review submission: the verdict event, an optional
/// summary body, and the anchored comments — plus the draft comments that were
/// skipped because their anchor no longer resolved.
#[derive(Debug, Clone)]
pub struct ReviewPayload {
    /// GitHub `event` token (`COMMENT` | `APPROVE` | `REQUEST_CHANGES`).
    pub event: &'static str,
    pub body: Option<String>,
    pub comments: Vec<SubmitComment>,
    pub skipped: Vec<SkippedComment>,
}

/// The GitHub `event` token for a [`ReviewVerdict`].
#[must_use]
pub fn verdict_event(v: ReviewVerdict) -> &'static str {
    match v {
        ReviewVerdict::Comment => "COMMENT",
        ReviewVerdict::Approve => "APPROVE",
        ReviewVerdict::RequestChanges => "REQUEST_CHANGES",
    }
}

/// Why a publish could not proceed (vs. an individual comment being skipped).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublishError {
    /// At least one draft comment was authored against a head SHA that differs
    /// from the PR's current head — the PR was force-pushed/rebased. Publishing
    /// would mis-place comments, so we refuse the whole batch with the SHAs so the
    /// reviewer can re-review against the new head.
    StaleHeadSha { selection_sha: String, diff_sha: String },
    /// A `COMMENT` review with no comments and no summary is empty — GitHub
    /// rejects it, and there is nothing to say.
    EmptyReview,
}

/// Resolve every draft comment against the current diff and assemble the single
/// batched submission. Pure over its inputs (no I/O), so the stale-SHA refusal
/// and skip-vs-include logic are exhaustively unit-testable.
///
/// # Errors
/// - [`PublishError::StaleHeadSha`] if any comment was made against a different
///   head SHA than the diff's current one (refuse rather than mis-place).
/// - [`PublishError::EmptyReview`] if the result would carry no comments and no
///   summary on a plain `COMMENT` verdict.
pub fn assemble_review_payload(
    draft: &ReviewDraftInfo,
    diff: &PullDiff,
    summary: Option<String>,
    expected_head_sha: Option<&str>,
) -> Result<ReviewPayload, PublishError> {
    // Force-push guard (docs §11): if the reviewer was viewing a different head
    // SHA than the PR is now at, the draft's line numbers no longer refer to the
    // same content. Refuse the whole batch rather than mis-place comments.
    if let Some(expected) = expected_head_sha
        && expected != diff.head_sha
    {
        return Err(PublishError::StaleHeadSha {
            selection_sha: expected.to_string(),
            diff_sha: diff.head_sha.clone(),
        });
    }

    let event = verdict_event(draft.verdict);
    let mut comments = Vec::new();
    let mut skipped = Vec::new();

    for c in &draft.comments {
        match resolve_comment(c, diff) {
            Ok(sc) => comments.push(sc),
            // A stale head SHA is fatal for the whole publish: the draft predates
            // a force-push and its line numbers no longer mean what they did.
            Err(AnchorError::StaleHeadSha { selection_sha, diff_sha }) => {
                return Err(PublishError::StaleHeadSha { selection_sha, diff_sha });
            }
            // The line vanished from the diff (or never resolved): skip just this
            // comment, report it, and keep the rest of the review.
            Err(reason) => skipped.push(SkippedComment {
                comment_id: c.id,
                path: c.path.clone(),
                line: c.line,
                reason,
            }),
        }
    }

    let body = summary.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    // GitHub rejects an empty COMMENT review (no body + no comments). Approve /
    // request-changes are valid with neither, so only guard the COMMENT case.
    if event == "COMMENT" && comments.is_empty() && body.is_none() {
        return Err(PublishError::EmptyReview);
    }

    Ok(ReviewPayload { event, body, comments, skipped })
}

/// Resolve one draft comment to a [`SubmitComment`] via the GH-VIEW-2 anchor.
fn resolve_comment(c: &DraftCommentInfo, diff: &PullDiff) -> Result<SubmitComment, AnchorError> {
    let sel = cctui_proto::github::DiffSelection {
        path: c.path.clone(),
        side: c.side,
        line: c.line,
        start_line: c.start_line,
        head_sha: diff.head_sha.clone(),
    };
    // Anchored against the diff's own head SHA; the force-push refusal is done up
    // front in `assemble_review_payload` against the reviewer's expected SHA.
    let anchor = crate::anchor::resolve(diff, &sel)?;
    Ok(SubmitComment {
        draft_comment_id: c.id,
        path: anchor.path,
        side: anchor.side.github_token().to_string(),
        line: anchor.line,
        start_line: anchor.start_line,
        start_side: anchor.start_side.map(|s| s.github_token().to_string()),
        body: c.body.clone(),
    })
}

/// The JSON body for `POST /repos/{o}/{r}/pulls/{n}/reviews`, built from an
/// assembled [`ReviewPayload`] and the commit the review anchors to.
#[must_use]
pub fn review_request_json(payload: &ReviewPayload, commit_id: &str) -> serde_json::Value {
    let comments: Vec<serde_json::Value> = payload
        .comments
        .iter()
        .map(|c| {
            let mut obj = serde_json::json!({
                "path": c.path,
                "line": c.line,
                "side": c.side,
                "body": c.body,
            });
            if let (Some(start_line), Some(start_side)) = (c.start_line, c.start_side.as_ref()) {
                obj["start_line"] = serde_json::json!(start_line);
                obj["start_side"] = serde_json::json!(start_side);
            }
            obj
        })
        .collect();
    let mut req = serde_json::json!({
        "commit_id": commit_id,
        "event": payload.event,
        "comments": comments,
    });
    if let Some(body) = &payload.body {
        req["body"] = serde_json::json!(body);
    }
    req
}

/// The result of submitting a review to GitHub: the new review id and the
/// per-comment GitHub ids in submission order (so they can be written back to the
/// originating draft comments).
#[derive(Debug, Clone)]
pub struct SubmittedReview {
    pub review_id: i64,
    /// GitHub comment ids in the SAME order as the submitted `comments`.
    pub comment_ids: Vec<i64>,
}

/// One pulled-down GitHub review thread (its identifying fields) + its comments.
#[derive(Debug, Clone)]
pub struct FetchedThread {
    pub thread_node_id: String,
    pub path: Option<String>,
    pub side: Option<String>,
    pub line: Option<i64>,
    pub resolved: bool,
    pub comments: Vec<FetchedThreadComment>,
}

/// One pulled-down GitHub review comment within a thread.
#[derive(Debug, Clone)]
pub struct FetchedThreadComment {
    pub comment_id: i64,
    pub author: String,
    pub body: String,
    pub created_at: String,
}

/// Abstracts the GitHub review round-trips so publish assembly + thread pull-down
/// are testable without a live API.
#[async_trait::async_trait]
pub trait ReviewSubmitClient: Send + Sync {
    /// Submit ONE batched review via `POST /repos/{o}/{r}/pulls/{n}/reviews`.
    async fn submit_review(
        &self,
        credential: &str,
        repo: &str,
        number: i64,
        body: &serde_json::Value,
    ) -> anyhow::Result<SubmittedReview>;

    /// Fetch the PR's existing OPEN review threads + their comments for pull-down
    /// (CONN-3 `github.review_threads` / `review_comments`).
    async fn fetch_review_threads(
        &self,
        credential: &str,
        repo: &str,
        number: i64,
    ) -> anyhow::Result<Vec<FetchedThread>>;
}

/// Production [`ReviewSubmitClient`] over GitHub's REST API. Reuses rustls via the
/// workspace `reqwest` (same auth pattern as [`crate::diff::HttpDiffClient`]).
pub struct HttpReviewClient {
    http: reqwest::Client,
}

impl Default for HttpReviewClient {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpReviewClient {
    #[must_use]
    pub fn new() -> Self {
        Self { http: reqwest::Client::new() }
    }
}

#[async_trait::async_trait]
impl ReviewSubmitClient for HttpReviewClient {
    async fn submit_review(
        &self,
        credential: &str,
        repo: &str,
        number: i64,
        body: &serde_json::Value,
    ) -> anyhow::Result<SubmittedReview> {
        let url = format!("{API_BASE}/repos/{repo}/pulls/{number}/reviews");
        let resp = self
            .http
            .post(&url)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(reqwest::header::AUTHORIZATION, format!("Bearer {credential}"))
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .json(body)
            .send()
            .await?;
        if !resp.status().is_success() {
            // The status is safe to surface; the request body (which carried no
            // secret) is not logged.
            anyhow::bail!("github submit review returned {}", resp.status());
        }
        let v: serde_json::Value = resp.json().await?;
        let review_id = v
            .get("id")
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| anyhow::anyhow!("github review response missing id"))?;
        // Fetch the review's comments to map them back to drafts in order.
        let comments_url =
            format!("{API_BASE}/repos/{repo}/pulls/{number}/reviews/{review_id}/comments");
        let comment_ids = match self
            .http
            .get(&comments_url)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header(reqwest::header::AUTHORIZATION, format!("Bearer {credential}"))
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => {
                let arr: serde_json::Value = r.json().await.unwrap_or_default();
                arr.as_array()
                    .map(|a| a.iter().filter_map(|c| c.get("id").and_then(serde_json::Value::as_i64)).collect())
                    .unwrap_or_default()
            }
            // The review posted; failing to read its comment ids just means we
            // can't backfill github_comment_id — non-fatal.
            _ => Vec::new(),
        };
        Ok(SubmittedReview { review_id, comment_ids })
    }

    async fn fetch_review_threads(
        &self,
        credential: &str,
        repo: &str,
        number: i64,
    ) -> anyhow::Result<Vec<FetchedThread>> {
        // The REST review-comments endpoint returns posted inline comments;
        // GitHub groups them into threads via `in_reply_to_id`. We reconstruct
        // threads by treating each top-level comment (no in_reply_to_id) as a
        // thread root keyed on its node id, attaching replies to it.
        let mut roots: Vec<FetchedThread> = Vec::new();
        let mut by_id: std::collections::HashMap<i64, usize> = std::collections::HashMap::new();
        let mut page = 1u32;
        loop {
            let url = format!("{API_BASE}/repos/{repo}/pulls/{number}/comments");
            let resp = self
                .http
                .get(&url)
                .query(&[("per_page", "100".to_string()), ("page", page.to_string())])
                .header(reqwest::header::USER_AGENT, USER_AGENT)
                .header(reqwest::header::AUTHORIZATION, format!("Bearer {credential}"))
                .header(reqwest::header::ACCEPT, "application/vnd.github+json")
                .send()
                .await?;
            if !resp.status().is_success() {
                anyhow::bail!("github pull comments returned {}", resp.status());
            }
            let body: serde_json::Value = resp.json().await?;
            let arr = body.as_array().cloned().unwrap_or_default();
            let n = arr.len();
            for c in &arr {
                let Some(comment_id) = c.get("id").and_then(serde_json::Value::as_i64) else {
                    continue;
                };
                let author = c
                    .get("user")
                    .and_then(|u| u.get("login"))
                    .and_then(|l| l.as_str())
                    .unwrap_or_default()
                    .to_string();
                let fc = FetchedThreadComment {
                    comment_id,
                    author,
                    body: c.get("body").and_then(|b| b.as_str()).unwrap_or_default().to_string(),
                    created_at: c
                        .get("created_at")
                        .and_then(|t| t.as_str())
                        .unwrap_or_default()
                        .to_string(),
                };
                let reply_to = c.get("in_reply_to_id").and_then(serde_json::Value::as_i64);
                if let Some(parent) = reply_to.and_then(|p| by_id.get(&p).copied()) {
                    roots[parent].comments.push(fc);
                } else {
                    by_id.insert(comment_id, roots.len());
                    roots.push(FetchedThread {
                        thread_node_id: c
                            .get("node_id")
                            .and_then(|s| s.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        path: c.get("path").and_then(|s| s.as_str()).map(str::to_string),
                        side: c.get("side").and_then(|s| s.as_str()).map(str::to_string),
                        line: c.get("line").and_then(serde_json::Value::as_i64),
                        resolved: false,
                        comments: vec![fc],
                    });
                }
            }
            if n < 100 {
                break;
            }
            page += 1;
        }
        Ok(roots)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cctui_proto::github::{
        DiffFile, DiffHunk, DiffLine, DiffLineKind, DiffSide, DraftAuthorKind, DraftStatus,
    };

    fn ctx(o: u32, n: u32) -> DiffLine {
        DiffLine { kind: DiffLineKind::Context, content: "x".into(), old_line: Some(o), new_line: Some(n) }
    }
    fn add(n: u32) -> DiffLine {
        DiffLine { kind: DiffLineKind::Add, content: "x".into(), old_line: None, new_line: Some(n) }
    }

    fn diff(head_sha: &str) -> PullDiff {
        let h = DiffHunk {
            old_start: 1,
            old_lines: 1,
            new_start: 1,
            new_lines: 2,
            header: None,
            lines: vec![ctx(1, 1), add(2)],
        };
        PullDiff {
            repo: "o/r".into(),
            number: 1,
            head_sha: head_sha.into(),
            total_files: 1,
            total_changes: 1,
            huge: false,
            files: vec![DiffFile {
                path: "a.rs".into(),
                previous_path: None,
                status: "modified".into(),
                additions: 1,
                deletions: 0,
                hunks: vec![h],
                truncated: false,
                binary: false,
                blob_sha: None,
            }],
        }
    }

    fn comment(line: u32, side: DiffSide, path: &str) -> DraftCommentInfo {
        DraftCommentInfo {
            id: uuid::Uuid::new_v4(),
            draft_id: uuid::Uuid::nil(),
            path: path.into(),
            side,
            line,
            start_line: None,
            body: "looks good".into(),
            github_comment_id: None,
            in_reply_to: None,
            created_at: "2026-06-17T00:00:00Z".into(),
            updated_at: "2026-06-17T00:00:00Z".into(),
        }
    }

    fn draft(verdict: ReviewVerdict, comments: Vec<DraftCommentInfo>) -> ReviewDraftInfo {
        ReviewDraftInfo {
            id: uuid::Uuid::nil(),
            connector_id: uuid::Uuid::nil(),
            repo: "o/r".into(),
            number: 1,
            author_kind: DraftAuthorKind::User,
            author_user_id: Some(uuid::Uuid::nil()),
            author_session_id: None,
            verdict,
            status: DraftStatus::Draft,
            created_at: "2026-06-17T00:00:00Z".into(),
            updated_at: "2026-06-17T00:00:00Z".into(),
            comments,
        }
    }

    #[test]
    fn assembles_batched_payload_with_anchored_comments() {
        let d = draft(ReviewVerdict::Comment, vec![comment(2, DiffSide::New, "a.rs")]);
        let p = assemble_review_payload(&d, &diff("sha1"), Some("ship it".into()), None).unwrap();
        assert_eq!(p.event, "COMMENT");
        assert_eq!(p.body.as_deref(), Some("ship it"));
        assert_eq!(p.comments.len(), 1);
        assert_eq!(p.comments[0].path, "a.rs");
        assert_eq!(p.comments[0].side, "RIGHT");
        assert_eq!(p.comments[0].line, 2);
        assert!(p.skipped.is_empty());
    }

    #[test]
    fn one_batched_request_carries_all_comments() {
        // Two comments → ONE request with a 2-element comments array (no spam).
        let d = draft(
            ReviewVerdict::RequestChanges,
            vec![comment(1, DiffSide::New, "a.rs"), comment(2, DiffSide::New, "a.rs")],
        );
        let p = assemble_review_payload(&d, &diff("sha1"), None, None).unwrap();
        let json = review_request_json(&p, "sha1");
        assert_eq!(json["event"], "REQUEST_CHANGES");
        assert_eq!(json["commit_id"], "sha1");
        assert_eq!(json["comments"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn unanchorable_comment_is_skipped_not_fatal() {
        // Line 999 isn't in the diff → skipped; the anchored one still posts.
        let d = draft(
            ReviewVerdict::Comment,
            vec![comment(2, DiffSide::New, "a.rs"), comment(999, DiffSide::New, "a.rs")],
        );
        let p = assemble_review_payload(&d, &diff("sha1"), None, None).unwrap();
        assert_eq!(p.comments.len(), 1);
        assert_eq!(p.skipped.len(), 1);
        assert_eq!(p.skipped[0].line, 999);
        assert_eq!(p.skipped[0].reason, AnchorError::LineNotInDiff);
    }

    #[test]
    fn comment_on_missing_file_is_skipped() {
        let d = draft(ReviewVerdict::Comment, vec![comment(2, DiffSide::New, "gone.rs")]);
        let p = assemble_review_payload(&d, &diff("sha1"), Some("summary".into()), None).unwrap();
        assert!(p.comments.is_empty());
        assert_eq!(p.skipped.len(), 1);
        assert_eq!(p.skipped[0].reason, AnchorError::FileNotFound);
    }

    #[test]
    fn empty_comment_review_with_no_summary_is_rejected() {
        let d = draft(ReviewVerdict::Comment, vec![]);
        assert_eq!(
            assemble_review_payload(&d, &diff("sha1"), None, None).unwrap_err(),
            PublishError::EmptyReview
        );
    }

    #[test]
    fn approve_with_no_comments_is_allowed() {
        let d = draft(ReviewVerdict::Approve, vec![]);
        let p = assemble_review_payload(&d, &diff("sha1"), None, None).unwrap();
        assert_eq!(p.event, "APPROVE");
        assert!(p.comments.is_empty());
    }

    #[test]
    fn multi_line_range_carries_start_fields_in_json() {
        let mut c = comment(2, DiffSide::New, "a.rs");
        c.start_line = Some(1);
        // line 1 is context (both sides), line 2 is add on New — both diffable.
        let d = draft(ReviewVerdict::Comment, vec![c]);
        let p = assemble_review_payload(&d, &diff("sha1"), None, None).unwrap();
        assert_eq!(p.comments[0].start_line, Some(1));
        assert_eq!(p.comments[0].start_side.as_deref(), Some("RIGHT"));
        let json = review_request_json(&p, "sha1");
        assert_eq!(json["comments"][0]["start_line"], 1);
        assert_eq!(json["comments"][0]["start_side"], "RIGHT");
    }

    #[test]
    fn stale_head_sha_refuses_the_whole_publish() {
        // Reviewer drafted against "sha-old"; the PR is now at "sha1" (force-push).
        // The publish must refuse rather than re-anchor onto rotated lines.
        let d = draft(ReviewVerdict::Comment, vec![comment(2, DiffSide::New, "a.rs")]);
        let err =
            assemble_review_payload(&d, &diff("sha1"), Some("ship".into()), Some("sha-old")).unwrap_err();
        assert_eq!(
            err,
            PublishError::StaleHeadSha {
                selection_sha: "sha-old".into(),
                diff_sha: "sha1".into(),
            }
        );
    }

    #[test]
    fn matching_expected_head_sha_publishes_normally() {
        let d = draft(ReviewVerdict::Comment, vec![comment(2, DiffSide::New, "a.rs")]);
        let p = assemble_review_payload(&d, &diff("sha1"), None, Some("sha1")).unwrap();
        assert_eq!(p.comments.len(), 1);
    }

    #[test]
    fn verdict_event_tokens() {
        assert_eq!(verdict_event(ReviewVerdict::Comment), "COMMENT");
        assert_eq!(verdict_event(ReviewVerdict::Approve), "APPROVE");
        assert_eq!(verdict_event(ReviewVerdict::RequestChanges), "REQUEST_CHANGES");
    }
}
