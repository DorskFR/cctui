//! GitHub integration wire types. They live here because ts-rs bindings are
//! generated from this crate and both the server and `cctui-github` depend on it.
//! Credentials are accepted on create only and never read back.

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum GithubCredentialKind {
    /// Fine-grained personal access token.
    Pat,
    AppInstallation,
}

/// Body for `POST /api/v1/github/connectors`. The only place the plaintext
/// credential and webhook secret travel; both are stored encrypted.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreateConnector {
    pub name: String,
    pub credential_kind: GithubCredentialKind,
    pub credential: String,
    /// `owner/name` slugs, or bare `owner` for a whole org.
    #[serde(default)]
    pub repos: Vec<String>,
    /// `X-Hub-Signature-256` secret.
    #[serde(default)]
    pub webhook_secret: Option<String>,
    /// Required with the admin token, ignored otherwise.
    #[serde(default)]
    pub user_id: Option<Uuid>,
}

/// Body for `PATCH /api/v1/github/connectors/{id}`; absent fields are unchanged.
/// Rotating the credential clears the cached viewer login.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UpdateConnector {
    #[serde(default)]
    pub name: Option<String>,
    /// Replaces the whole list; empty tracks every repo the token can see.
    #[serde(default)]
    pub repos: Option<Vec<String>>,
    /// Omitted or empty keeps the current one.
    #[serde(default)]
    pub credential: Option<String>,
    /// `Some("")` clears it, `None` leaves it unchanged.
    #[serde(default)]
    pub webhook_secret: Option<String>,
}

/// A connector as read back; secrets are never included.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ConnectorInfo {
    pub id: Uuid,
    pub name: String,
    pub credential_kind: GithubCredentialKind,
    /// Masked fragment, e.g. `github_pat_ab…wxyz`.
    pub credential_preview: String,
    pub has_webhook_secret: bool,
    pub repos: Vec<String>,
    pub user_id: Uuid,
    /// ISO-8601.
    pub created_at: String,
    /// ISO-8601. `None` until first polled.
    pub last_polled_at: Option<String>,
    /// `None` when the last poll succeeded.
    pub last_error: Option<String>,
}

/// Parsed PR, keyed on `(connector, repo, number)`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PullUpsert {
    /// GraphQL node id.
    pub node_id: String,
    pub repo: String,
    pub number: i64,
    pub title: String,
    /// `open` | `closed`.
    pub state: String,
    pub merged: bool,
    pub draft: bool,
    /// `None` until GitHub computes it.
    pub mergeable_state: Option<String>,
    pub author: String,
    pub head_sha: String,
    pub base_ref: String,
    pub head_ref: String,
    /// ISO-8601.
    pub gh_created_at: String,
    /// ISO-8601.
    pub gh_updated_at: String,
}

/// Parsed `check_run` or legacy commit status.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CheckUpsert {
    pub repo: String,
    pub head_sha: String,
    /// `check_run` id, or `status:<context>` for a commit status.
    pub external_id: String,
    pub name: String,
    /// `queued` | `in_progress` | `completed`.
    pub status: String,
    /// `None` while running.
    pub conclusion: Option<String>,
    pub details_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ReviewUpsert {
    pub repo: String,
    pub pull_number: i64,
    pub review_id: i64,
    pub reviewer: String,
    /// `approved` | `changes_requested` | `commented` | `dismissed` | `pending`.
    pub state: String,
    pub body: Option<String>,
    pub commit_id: Option<String>,
    /// ISO-8601. `None` while pending.
    pub submitted_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ReviewThreadUpsert {
    pub repo: String,
    pub pull_number: i64,
    pub thread_node_id: String,
    pub path: String,
    /// `LEFT` | `RIGHT`, when anchored.
    pub side: Option<String>,
    pub line: Option<i64>,
    pub resolved: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ReviewCommentUpsert {
    pub repo: String,
    pub pull_number: i64,
    pub comment_id: i64,
    pub thread_node_id: Option<String>,
    pub author: String,
    pub body: String,
    pub path: Option<String>,
    pub side: Option<String>,
    pub line: Option<i64>,
    pub gh_created_at: String,
    pub gh_updated_at: String,
}

/// Object kind a [`crate::ws::ServerEvent::GithubEvent`] refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum GithubEventKind {
    Pull,
    Check,
    Review,
    ReviewThread,
    ReviewComment,
}

/// Locator for what to refetch; never carries row bodies or credentials.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GithubEventPayload {
    pub connector_id: Uuid,
    pub repo: String,
    /// `None` for SHA-keyed checks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_number: Option<i64>,
}

/// What a tracked PR needs from the viewer. Exactly one per PR; variants are
/// in inbox display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum AttentionBucket {
    /// Review requested from the viewer and not yet given.
    NeedsMyReview,
    MyPrChangesRequested,
    MyPrCiRed,
    /// Green or no CI and no outstanding change requests.
    MyPrMergeable,
    /// Nothing actionable.
    Waiting,
}

impl AttentionBucket {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::NeedsMyReview => "Needs my review",
            Self::MyPrChangesRequested => "My PR — changes requested",
            Self::MyPrCiRed => "My PR — CI red",
            Self::MyPrMergeable => "My PR — mergeable",
            Self::Waiting => "Waiting",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CheckSummary {
    /// Includes neutral and skipped.
    pub passed: u32,
    pub failed: u32,
    pub pending: u32,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ReviewSummary {
    pub changes_requested: u32,
    pub approved: u32,
    pub commented: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    /// Without the leading ` `/`+`/`-` marker.
    pub content: String,
    /// 1-based. `None` for an added line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_line: Option<u32>,
    /// 1-based. `None` for a deleted line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_line: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum DiffLineKind {
    Context,
    Add,
    Del,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiffHunk {
    /// 1-based.
    pub old_start: u32,
    pub old_lines: u32,
    /// 1-based.
    pub new_start: u32,
    pub new_lines: u32,
    /// Text after the second `@@`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiffFile {
    /// Head-side path; the removed path for a delete.
    pub path: String,
    /// Set for renames.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_path: Option<String>,
    /// `added` | `modified` | `removed` | `renamed` | `copied` | `changed` | `unchanged`.
    pub status: String,
    pub additions: u32,
    pub deletions: u32,
    /// Empty for binaries and for `truncated` files.
    pub hunks: Vec<DiffHunk>,
    /// GitHub omitted the patch and the blob fallback could not rebuild it.
    pub truncated: bool,
    pub binary: bool,
    /// Head-side blob SHA; keys "reviewed" marks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob_sha: Option<String>,
}

/// Structured PR diff, cached server-side by `head_sha`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PullDiff {
    pub repo: String,
    pub number: i64,
    pub head_sha: String,
    /// Whole-PR count, even when `huge` caps `files`.
    pub total_files: u32,
    /// Additions + deletions.
    pub total_changes: u64,
    /// Over the large-diff threshold; `files` is capped.
    pub huge: bool,
    pub files: Vec<DiffFile>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum DiffSide {
    /// Base side, GitHub `LEFT`.
    Old,
    /// Head side, GitHub `RIGHT`.
    New,
}

impl DiffSide {
    #[must_use]
    pub const fn github_token(self) -> &'static str {
        match self {
            Self::Old => "LEFT",
            Self::New => "RIGHT",
        }
    }
}

/// A reviewer's diff selection. A different diff `head_sha` makes it stale.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiffSelection {
    /// Head-side path.
    pub path: String,
    pub side: DiffSide,
    /// 1-based, on `side`.
    pub line: u32,
    /// Inclusive range start on the same side, `<= line`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u32>,
    pub head_sha: String,
}

/// A [`DiffSelection`] resolved to GitHub review-comment coordinates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CommentAnchor {
    pub path: String,
    pub commit_id: String,
    /// End line of a range.
    pub line: u32,
    pub side: DiffSide,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u32>,
    /// Always equals `side` when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_side: Option<DiffSide>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum AnchorError {
    /// The PR head moved since the selection was made.
    StaleHeadSha {
        selection_sha: String,
        diff_sha: String,
    },
    FileNotFound,
    /// GitHub only accepts comments on lines in the diff.
    LineNotInDiff,
    /// `start_line > line`, or endpoints not diffable on one side.
    InvalidRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum DraftAuthorKind {
    /// One open draft per user and PR.
    User,
    Agent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ReviewVerdict {
    Comment,
    Approve,
    RequestChanges,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum DraftStatus {
    Draft,
    Published,
}

/// Opens the caller's draft for a PR, or returns the open one.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreateReviewDraft {
    /// Defaults to `comment`.
    #[serde(default)]
    pub verdict: Option<ReviewVerdict>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UpdateReviewDraft {
    pub verdict: ReviewVerdict,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreateDraftComment {
    pub path: String,
    pub side: DiffSide,
    /// End line of a range.
    pub line: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u32>,
    pub body: String,
    /// Parent GitHub comment id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_reply_to: Option<i64>,
}

/// Only the body is editable; the anchor is fixed.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UpdateDraftComment {
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DraftCommentInfo {
    pub id: Uuid,
    pub draft_id: Uuid,
    pub path: String,
    pub side: DiffSide,
    pub line: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u32>,
    pub body: String,
    /// `None` until published.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_comment_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_reply_to: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ReviewDraftInfo {
    pub id: Uuid,
    pub connector_id: Uuid,
    pub repo: String,
    pub number: i64,
    pub author_kind: DraftAuthorKind,
    /// Set when `author_kind` is `user`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_user_id: Option<Uuid>,
    /// Set when `author_kind` is `agent`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_session_id: Option<String>,
    pub verdict: ReviewVerdict,
    pub status: DraftStatus,
    pub created_at: String,
    pub updated_at: String,
    /// Oldest first.
    pub comments: Vec<DraftCommentInfo>,
}

/// Body for `mark-viewed` / `unmark-viewed`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MarkViewedRequest {
    pub path: String,
    /// Required on mark, ignored on unmark.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob_sha: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ViewedMarkInfo {
    pub path: String,
    /// The file counts as reviewed only while its current blob SHA matches.
    pub blob_sha: String,
    pub marked_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PullInboxItem {
    pub connector_id: Uuid,
    pub repo: String,
    pub number: i64,
    pub title: String,
    /// `open` | `closed`.
    pub state: String,
    pub merged: bool,
    pub draft: bool,
    pub author: String,
    pub head_ref: String,
    pub base_ref: String,
    pub mergeable_state: Option<String>,
    /// ISO-8601; the inbox sort key.
    pub gh_updated_at: String,
    pub bucket: AttentionBucket,
    pub checks: CheckSummary,
    pub reviews: ReviewSummary,
}

/// Publishes a draft as one batched GitHub review.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PublishReviewRequest {
    pub draft_id: Uuid,
    /// Review body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Refuse to publish if the PR head differs. `None` skips the check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_head_sha: Option<String>,
}

/// A draft comment left out because it no longer anchors.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SkippedComment {
    pub comment_id: Uuid,
    pub path: String,
    pub line: u32,
    pub reason: AnchorError,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PublishReviewResult {
    pub review_id: i64,
    pub submitted: u32,
    pub skipped: Vec<SkippedComment>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ReviewThreadCommentInfo {
    pub comment_id: i64,
    pub author: String,
    pub body: String,
    pub created_at: String,
}

/// A review thread already posted on GitHub.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ReviewThreadInfo {
    pub thread_node_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// `LEFT` | `RIGHT`, when anchored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side: Option<String>,
    /// 1-based, on `side`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<i64>,
    pub resolved: bool,
    /// Oldest first.
    pub comments: Vec<ReviewThreadCommentInfo>,
}
