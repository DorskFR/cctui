//! Wire types for the GitHub integration (GH-CONN-1).
//!
//! These live in `cctui-proto` rather than the `cctui-github` crate for two
//! reasons: (1) ts-rs binding generation runs against `cctui-proto` +
//! `cctui-server` only, so types needing TypeScript bindings must live here;
//! (2) the server's auth layer and the GitHub crate both need [`CallerIdentity`],
//! and proto is the one crate both depend on without a dependency cycle.
//!
//! The encrypted credential never appears in any of these types — a connector is
//! created with a plaintext credential (request only) and always read back with
//! the credential masked. See `cctui-github` for the at-rest encryption.

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

/// The authenticated caller, in terms the `cctui-github` crate can consume
/// without depending on `cctui-server` (where the richer `AuthContext` lives).
///
/// The server derives this from its `AuthContext` in a thin middleware applied
/// to the nested GitHub router, then inserts it as a request extension. The
/// GitHub handlers extract it to scope connector rows to their owner — a user
/// acts as itself; the admin token has no user identity and must name the owner
/// explicitly, mirroring the OAuth-account vault (CCT-251).
#[derive(Debug, Clone)]
pub struct CallerIdentity {
    /// The owning user, when the caller is a user/machine token. `None` for the
    /// env admin token, which has no user identity of its own.
    pub user_id: Option<Uuid>,
    /// Whether the caller authenticated with the env admin token.
    pub is_admin: bool,
}

/// Which credential kind a connector holds. Recorded so the UI can label the
/// connector and so a future webhook/reconcile path knows how to use the token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum GithubCredentialKind {
    /// A fine-grained personal access token (the MVP path, single identity).
    Pat,
    /// A GitHub App installation token.
    AppInstallation,
}

/// Request body for `POST /api/v1/github/connectors`.
///
/// Carries the **plaintext** credential and webhook secret exactly once, on
/// create. The server encrypts both at rest (`crate::crypto` XOR-vault pattern,
/// same key as the OAuth-account vault) and never echoes them back — every read
/// path returns [`ConnectorInfo`] with the credential masked.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreateConnector {
    /// Human-readable label for this connector (e.g. `personal`, `work`).
    pub name: String,
    /// Whether `credential` is a PAT or an App installation token.
    pub credential_kind: GithubCredentialKind,
    /// The GitHub credential. Stored encrypted; never returned by any endpoint.
    pub credential: String,
    /// `owner/name` slugs (or bare `owner` for whole-org) this connector tracks.
    #[serde(default)]
    pub repos: Vec<String>,
    /// Optional webhook signing secret (`X-Hub-Signature-256`). Stored
    /// encrypted; never returned. A later story verifies signatures with it.
    #[serde(default)]
    pub webhook_secret: Option<String>,
    /// Owning user — required (and only honoured) when authenticated with the
    /// admin token, which has no user identity of its own.
    #[serde(default)]
    pub user_id: Option<Uuid>,
}

/// Request body for `PATCH /api/v1/github/connectors/{id}`.
///
/// Every field is optional: only the present ones are changed (rename, re-scope
/// repos, rotate the credential, set/clear the webhook secret). A `credential`
/// is only re-encrypted when present and non-empty — omit it to keep the stored
/// one. Rotating the credential clears the cached `viewer_login` so the next
/// poll re-resolves it against the new token.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UpdateConnector {
    /// New human-readable label, if renaming.
    #[serde(default)]
    pub name: Option<String>,
    /// Replacement set of `owner/name` slugs, if re-scoping. Replaces the whole
    /// list (not merged); an empty list tracks every repo the token can see.
    #[serde(default)]
    pub repos: Option<Vec<String>>,
    /// A replacement credential to rotate to. Omitted/empty = keep the current.
    #[serde(default)]
    pub credential: Option<String>,
    /// Webhook secret: `Some(non-empty)` sets it, `Some("")` clears it, `None`
    /// leaves it unchanged.
    #[serde(default)]
    pub webhook_secret: Option<String>,
}

/// API view of a connector. The credential and webhook secret are **never**
/// present — only a non-secret [`ConnectorInfo::credential_preview`] mask, so the
/// webui and agents can confirm a connector exists without ever seeing the token.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ConnectorInfo {
    pub id: Uuid,
    pub name: String,
    pub credential_kind: GithubCredentialKind,
    /// A masked, non-secret fragment of the stored credential (e.g.
    /// `github_pat_ab…wxyz`). Enough to tell connectors apart; never the token.
    pub credential_preview: String,
    /// Whether a webhook secret is configured (the secret itself is never shown).
    pub has_webhook_secret: bool,
    /// `owner/name` slugs this connector tracks.
    pub repos: Vec<String>,
    /// Owning user — admins manage connectors across users, so the owner matters.
    pub user_id: Uuid,
    /// ISO-8601 creation timestamp.
    pub created_at: String,
    /// ISO-8601 time of the last reconcile-poll attempt, or `None` if the
    /// connector has not been polled yet (CCT-396).
    pub last_polled_at: Option<String>,
    /// The last reconcile-poll error (e.g. a bad/insufficient-scope PAT), or
    /// `None` when the last poll succeeded. Surfaced in the connector list so a
    /// misconfigured credential is visible without reading the server log.
    pub last_error: Option<String>,
}

// ---------------------------------------------------------------------------
// GH-CONN-3: synced PR state, CI checks, and posted reviews/threads/comments.
//
// These types are the parsed shape the webhook (GH-CONN-2) and reconcile
// (GH-CONN-4) paths produce and hand to `cctui-github`'s typed upsert
// functions, plus the API row views the inbox (GH-UI-1) reads back. They carry
// GitHub's own ids so upserts are idempotent regardless of which path observed
// the change first. No credential or webhook payload is ever represented here.
// ---------------------------------------------------------------------------

/// A parsed pull request, ready to upsert into `github.pulls`.
///
/// Produced by both the webhook (`pull_request` event) and the reconcile poll;
/// the upsert keys on `(connector, repo, number)` so they converge on one row.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PullUpsert {
    /// GitHub's stable global node id (GraphQL) for later API calls.
    pub node_id: String,
    /// `owner/name` slug the PR lives in.
    pub repo: String,
    /// The human-facing PR number within the repo.
    pub number: i64,
    pub title: String,
    /// `open` | `closed`.
    pub state: String,
    pub merged: bool,
    pub draft: bool,
    /// GitHub's `mergeable_state`; `None` until GitHub computes it.
    pub mergeable_state: Option<String>,
    pub author: String,
    /// Head commit SHA; CI checks key off this.
    pub head_sha: String,
    pub base_ref: String,
    pub head_ref: String,
    /// GitHub's own creation timestamp (ISO-8601).
    pub gh_created_at: String,
    /// GitHub's own last-update timestamp (ISO-8601).
    pub gh_updated_at: String,
}

/// A parsed CI check (check_run or legacy commit status) for a head SHA.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CheckUpsert {
    pub repo: String,
    pub head_sha: String,
    /// GitHub's check_run id, or `status:<context>` for legacy commit statuses.
    pub external_id: String,
    pub name: String,
    /// `queued` | `in_progress` | `completed`.
    pub status: String,
    /// `success` | `failure` | … ; `None` while running.
    pub conclusion: Option<String>,
    pub details_url: Option<String>,
}

/// A parsed submitted PR review (the posted side).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ReviewUpsert {
    pub repo: String,
    pub pull_number: i64,
    /// GitHub's review id (the upsert key, scoped to the connector).
    pub review_id: i64,
    pub reviewer: String,
    /// `approved` | `changes_requested` | `commented` | `dismissed` | `pending`.
    pub state: String,
    pub body: Option<String>,
    pub commit_id: Option<String>,
    /// ISO-8601; `None` for a pending review.
    pub submitted_at: Option<String>,
}

/// A parsed review thread (a conversation anchored on a diff line).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ReviewThreadUpsert {
    pub repo: String,
    pub pull_number: i64,
    /// GitHub's review-thread node id (the upsert key).
    pub thread_node_id: String,
    pub path: String,
    /// `LEFT` | `RIGHT` diff side, when anchored.
    pub side: Option<String>,
    pub line: Option<i64>,
    pub resolved: bool,
}

/// A parsed individual review comment.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ReviewCommentUpsert {
    pub repo: String,
    pub pull_number: i64,
    /// GitHub's review-comment id (the upsert key, scoped to the connector).
    pub comment_id: i64,
    /// Correlates to a `review_threads` row when known.
    pub thread_node_id: Option<String>,
    pub author: String,
    pub body: String,
    pub path: Option<String>,
    pub side: Option<String>,
    pub line: Option<i64>,
    pub gh_created_at: String,
    pub gh_updated_at: String,
}

/// Which GitHub object kind a [`crate::ws::ServerEvent::GithubEvent`] is about.
///
/// One value per `*Upsert` store function, so the inbox can route a change to
/// the right view without inspecting the payload shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum GithubEventKind {
    /// A pull request's tracked state changed (`github.pulls`).
    Pull,
    /// A CI check's status/conclusion changed (`github.checks`).
    Check,
    /// A submitted review changed (`github.reviews`).
    Review,
    /// A review thread changed (`github.review_threads`).
    ReviewThread,
    /// A review comment changed (`github.review_comments`).
    ReviewComment,
}

/// Credential-free locator for the GitHub object a
/// [`crate::ws::ServerEvent::GithubEvent`] refers to.
///
/// Deliberately minimal: enough for the client to know which PR/repo to
/// refetch over HTTP, never the row body, a token, or a raw webhook payload.
/// `pull_number` is `None` for object kinds (currently `Check`) keyed on a head
/// SHA rather than a PR number; the client maps the SHA to a PR via its cache.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GithubEventPayload {
    /// The connector that produced the change, so multi-account clients can
    /// scope the refresh.
    pub connector_id: Uuid,
    /// `owner/name` slug the object lives in.
    pub repo: String,
    /// The affected PR number, when the object is PR-scoped. `None` for
    /// SHA-keyed checks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_number: Option<i64>,
}

// ---------------------------------------------------------------------------
// GH-CONN-6: per-PR attention bucket.
//
// The connector derives one bucket per tracked PR (mirroring the session
// classifier's [`crate::classifier::Bucket`] vocabulary) so the `/github`
// inbox (GH-UI-1) can group PRs by "what do I need to do about this". The
// derivation is pure logic over the synced PR state + its checks + reviews +
// the viewer's relationship to the PR (authored / review-requested); see
// `cctui-github`'s `attention` module for the rules and tests.
// ---------------------------------------------------------------------------

/// What attention a tracked PR needs from the viewer, mirroring the session
/// classifier's bucket vocabulary (docs/github-integration.md §6.1).
///
/// Serialized `snake_case` so the webui inbox groups on the on-wire token. A
/// PR lands in exactly one bucket; the derivation (in `cctui-github`) picks the
/// single most-actionable one. The order here is the natural priority the inbox
/// renders top-to-bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum AttentionBucket {
    /// Someone else's PR where the viewer (or a team they're on) is
    /// review-requested and hasn't yet reviewed — the viewer owes a review.
    NeedsMyReview,
    /// The viewer's own PR a reviewer asked changes on — the ball is back with
    /// the viewer to address feedback.
    MyPrChangesRequested,
    /// The viewer's own PR with at least one failing CI check — fix the build.
    MyPrCiRed,
    /// The viewer's own PR with green (or no) CI and no outstanding
    /// change-requests — ready to merge (or chase the last approval).
    MyPrMergeable,
    /// Nothing actionable right now: closed/merged PRs, others' PRs the viewer
    /// isn't reviewing, or the viewer's PR still waiting on others' review.
    Waiting,
}

impl AttentionBucket {
    /// Stable label suitable for UI rendering / inbox section headers.
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

// ---------------------------------------------------------------------------
// GH-UI-1: the `/github` PR-inbox API view.
//
// `GET /api/v1/github/pulls` reads the synced CONN-3 rows back, derives each
// PR's attention bucket (GH-CONN-6) and a small CI/review summary, and returns
// one flat list the webui groups by `bucket`. No credential or raw payload is
// represented; the summaries are pre-aggregated so the inbox renders a row
// without a second round-trip.
// ---------------------------------------------------------------------------

/// Aggregated CI state for a PR's head SHA — the counts a row badge needs
/// without shipping every individual check.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CheckSummary {
    /// Checks that completed successfully (or neutral/skipped).
    pub passed: u32,
    /// Checks that completed with a failing conclusion.
    pub failed: u32,
    /// Checks not yet completed (`queued` / `in_progress`).
    pub pending: u32,
}

/// Aggregated review state for a PR — the strongest outstanding signal plus
/// raw counts, mirroring the classifier's collapse.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ReviewSummary {
    /// Number of submitted reviews requesting changes.
    pub changes_requested: u32,
    /// Number of submitted approvals.
    pub approved: u32,
    /// Number of plain comment reviews.
    pub commented: u32,
}

// ---------------------------------------------------------------------------
// GH-VIEW-1: the structured diff the server proxies from GitHub.
//
// `GET /api/v1/github/pulls/{connector_id}/{owner}/{name}/{number}/diff` fetches
// the PR's changed files from GitHub (paginated `pulls/{n}/files`, with a blob
// fallback for files GitHub truncates), parses each file's unified `patch` into
// hunks → lines, and returns this structured tree. The webui (GH-VIEW-3)
// virtualizes it. No daemon, no checkout: the data source is GitHub only
// (docs/github-integration.md §6.2). The result is cached per head SHA, so a
// repeated load of an unchanged PR costs no GitHub round-trip.
//
// No credential or raw token is ever represented here — only the diff content.
// ---------------------------------------------------------------------------

/// One line within a diff hunk.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiffLine {
    /// `context` (unchanged) | `add` | `del`.
    pub kind: DiffLineKind,
    /// Line text **without** the leading ` `/`+`/`-` marker.
    pub content: String,
    /// 1-based line number on the old (base) side; `None` for an added line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_line: Option<u32>,
    /// 1-based line number on the new (head) side; `None` for a deleted line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_line: Option<u32>,
}

/// The role of a [`DiffLine`] within its hunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum DiffLineKind {
    /// An unchanged context line (present on both sides).
    Context,
    /// A line added on the head side.
    Add,
    /// A line removed from the base side.
    Del,
}

/// One hunk (`@@ -a,b +c,d @@`) of a file's diff.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiffHunk {
    /// 1-based starting line on the old (base) side.
    pub old_start: u32,
    /// Number of old-side lines the hunk covers.
    pub old_lines: u32,
    /// 1-based starting line on the new (head) side.
    pub new_start: u32,
    /// Number of new-side lines the hunk covers.
    pub new_lines: u32,
    /// The hunk's section heading (the text after the second `@@`), when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    pub lines: Vec<DiffLine>,
}

/// One changed file in a PR diff: its path(s), change status, and parsed hunks.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiffFile {
    /// Current path (head side). For a delete this is the removed path.
    pub path: String,
    /// Previous path when the file was renamed/moved; `None` otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_path: Option<String>,
    /// GitHub's `status`: `added` | `modified` | `removed` | `renamed` |
    /// `copied` | `changed` | `unchanged`.
    pub status: String,
    pub additions: u32,
    pub deletions: u32,
    /// Parsed hunks. Empty for a binary file or a file whose patch could not be
    /// fetched even via the blob fallback (`truncated` is then `true`).
    pub hunks: Vec<DiffHunk>,
    /// GitHub flagged the inline patch as omitted (too large) **and** the blob
    /// fallback did not (or could not) reconstruct it — the webui shows a
    /// "load full file" affordance rather than a misleading empty diff.
    pub truncated: bool,
    /// A binary file has no textual patch; the webui renders a binary badge.
    pub binary: bool,
    /// The file's blob SHA on the head side (GitHub's `sha`), for blob-keyed
    /// "reviewed" marks (GH-VIEW-6) and the blob fallback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob_sha: Option<String>,
}

/// The structured diff for one PR, returned by `pulls/{ref}/diff`.
///
/// Cached server-side keyed on `head_sha`, so a repeated load of an unchanged
/// PR is served from memory with no GitHub round-trip (docs §6.2). When the head
/// SHA rotates (a new push), the cache entry is naturally superseded.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PullDiff {
    pub repo: String,
    pub number: i64,
    /// The head SHA this diff was computed against (the cache key).
    pub head_sha: String,
    /// Total changed-file count across the whole PR (even when `huge` truncates
    /// `files`), so the UI can show "showing N of M files".
    pub total_files: u32,
    /// Total changed-line count (additions + deletions) across the PR.
    pub total_changes: u64,
    /// `true` when the PR exceeds the large-diff threshold (docs §11, the
    /// 100k-plus-line case GitHub serves unreliably): `files` is then capped and
    /// the webui shows a "huge diff" affordance / per-file lazy load instead of
    /// rendering everything at once.
    pub huge: bool,
    pub files: Vec<DiffFile>,
}

// ---------------------------------------------------------------------------
// GH-VIEW-2: comment anchoring (rendered diff → GitHub review-comment coords).
//
// A reviewer selects a line (or a range) in the rendered diff; the webui draft
// UI (GH-VIEW-4) stores that selection as a [`DiffSelection`] and the publish
// path (GH-VIEW-5) turns it into a [`CommentAnchor`] — exactly the
// `line`/`side`/`start_line`/`start_side`/`commit_id` shape GitHub's
// `POST /repos/{o}/{r}/pulls/{n}/reviews` comments expect. Both the draft store
// and the publisher share these types so a comment lands on the SAME line it was
// drafted against. The anchoring logic itself lives in `cctui-github::anchor`.
//
// No credential or token is represented here — only diff coordinates.
// ---------------------------------------------------------------------------

/// Which side of the diff a selected line lives on, in cctui's own vocabulary
/// (a rendered split/unified view has an old column and a new column). Maps to
/// GitHub's `LEFT` (base/old) and `RIGHT` (head/new) review-comment sides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum DiffSide {
    /// The base (old) side — a deleted or unchanged line's old-side number.
    /// GitHub calls this `LEFT`.
    Old,
    /// The head (new) side — an added or unchanged line's new-side number.
    /// GitHub calls this `RIGHT`.
    New,
}

impl DiffSide {
    /// GitHub's review-comment `side` token (`LEFT`/`RIGHT`).
    #[must_use]
    pub fn github_token(self) -> &'static str {
        match self {
            DiffSide::Old => "LEFT",
            DiffSide::New => "RIGHT",
        }
    }
}

/// A reviewer's selection in the rendered diff, before it is resolved to a
/// GitHub anchor. This is what the draft UI (GH-VIEW-4) persists per comment:
/// the file path, the side, and the (display) line — optionally a multi-line
/// range whose `start_line..=line` is inclusive. `head_sha` records the SHA the
/// selection was made against, so a later force-push (head SHA rotated) can
/// invalidate stale anchors (docs §11).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiffSelection {
    /// The file's head-side path (the current path; for a rename this is the new
    /// name — GitHub anchors comments on the new path).
    pub path: String,
    /// Which side the selected line is on.
    pub side: DiffSide,
    /// 1-based line number on the selected side (old-side number when `side` is
    /// `Old`, new-side number when `New`).
    pub line: u32,
    /// For a multi-line selection, the (inclusive) start line on the same side.
    /// `None` for a single-line comment. Must be `<= line` and on the same side.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u32>,
    /// The head SHA the selection was made against. A diff whose `head_sha`
    /// differs (force-push) makes this selection stale — see [`anchor`] resolve.
    pub head_sha: String,
}

/// A fully resolved GitHub review-comment anchor — the precise shape a
/// `POST .../reviews` comment entry needs. Produced by resolving a
/// [`DiffSelection`] against the [`PullDiff`] it targets. Every field maps 1:1
/// to GitHub's review-comment API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CommentAnchor {
    /// GitHub comment `path` (head-side path).
    pub path: String,
    /// GitHub `commit_id` — the head SHA the comment is anchored to.
    pub commit_id: String,
    /// GitHub `line` — the (1-based) line on `side`. For a multi-line comment
    /// this is the END line of the range.
    pub line: u32,
    /// GitHub `side` — `LEFT` (base) or `RIGHT` (head).
    pub side: DiffSide,
    /// GitHub `start_line` — the START line of a multi-line range. `None` for a
    /// single-line comment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u32>,
    /// GitHub `start_side` — the side of `start_line`. Always equals `side`
    /// here (cctui never anchors a range across the two columns). `None` when
    /// `start_line` is `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_side: Option<DiffSide>,
}

/// Why a [`DiffSelection`] could not be resolved to a [`CommentAnchor`]. The
/// webui surfaces these so a reviewer knows a draft is un-anchorable rather than
/// silently publishing it onto the wrong line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum AnchorError {
    /// The diff's `head_sha` differs from the selection's — the PR was
    /// force-pushed (or otherwise re-based) since the draft was made, so the
    /// line numbers no longer refer to the same content (docs §11).
    StaleHeadSha {
        /// The SHA the selection was made against.
        selection_sha: String,
        /// The SHA the diff is now at.
        diff_sha: String,
    },
    /// No file in the diff matches the selection's `path`.
    FileNotFound,
    /// The selected line is not inside any hunk of the file (GitHub only accepts
    /// comments on lines that appear in the diff).
    LineNotInDiff,
    /// A multi-line range whose `start_line` is greater than `line`, or whose
    /// endpoints are not both diffable on the same side.
    InvalidRange,
}

// ---------------------------------------------------------------------------
// GH-VIEW-4: the native review-draft store (docs/github-integration.md §6.2).
//
// A reviewer adds inline comments **instantly** — no GitHub round-trip — into a
// local draft, anchored on the GH-VIEW-2 (path, side, line[, start_line])
// coordinates, then refines them before GH-VIEW-5 publishes the open draft as
// one batched `POST .../reviews`. These types are the wire shape of the draft
// CRUD routes under `/api/v1/github`. No credential or token is represented.
// ---------------------------------------------------------------------------

/// Who authored a review draft: a human reviewer or a review agent session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum DraftAuthorKind {
    /// A human reviewer (the owning user). One open draft per user+pull.
    User,
    /// A review agent — `session_id` names the cctui session that wrote it
    /// (GH-AGENT-2's MCP review tool stages drafts this way).
    Agent,
}

/// The pending review verdict a draft will submit when published (GH-VIEW-5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ReviewVerdict {
    /// Plain comments, no approval state change (GitHub `COMMENT`).
    Comment,
    /// Approve the PR (GitHub `APPROVE`).
    Approve,
    /// Request changes (GitHub `REQUEST_CHANGES`).
    RequestChanges,
}

/// Whether a draft is still local/editable or has been published to GitHub.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum DraftStatus {
    /// Local-only, still editable (the inline-commenting state GH-VIEW-4 owns).
    Draft,
    /// Submitted to GitHub as one batched review (GH-VIEW-5).
    Published,
}

/// Request body for `POST /api/v1/github/pulls/{connector_id}/{owner}/{name}/{number}/drafts`
/// — open (or reuse) the caller's draft for a PR.
///
/// The verdict defaults to `comment`; a user reusing their open draft keeps the
/// existing row (one open draft per user+pull). The PR ref is taken from the path.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreateReviewDraft {
    /// The pending verdict; defaults to `comment` when omitted.
    #[serde(default)]
    pub verdict: Option<ReviewVerdict>,
}

/// Patch body for `PATCH .../drafts/{draft_id}` — change the verdict.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UpdateReviewDraft {
    pub verdict: ReviewVerdict,
}

/// Request body for `POST .../drafts/{draft_id}/comments` — add one inline draft
/// comment anchored on the reviewer's diff selection (GH-VIEW-2 coordinates).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CreateDraftComment {
    /// Head-side file path the comment anchors on.
    pub path: String,
    /// Which side the selected line lives on.
    pub side: DiffSide,
    /// 1-based line on `side` (the END line for a multi-line range).
    pub line: u32,
    /// Inclusive start line for a multi-line range; `None` for a single line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u32>,
    /// The comment text.
    pub body: String,
    /// When replying to an existing GitHub thread, the parent comment id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_reply_to: Option<i64>,
}

/// Patch body for `PATCH .../drafts/{draft_id}/comments/{comment_id}` — edit the
/// body of an existing draft comment in place (the anchor is immutable).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UpdateDraftComment {
    pub body: String,
}

/// One inline draft comment, anchored to a diff line.
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
    /// GitHub's comment id once published; `None` while still a draft.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_comment_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_reply_to: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
}

/// API view of a review draft plus its inline comments, returned by the draft
/// CRUD routes. The webui renders the comments inline in the diff viewer.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ReviewDraftInfo {
    pub id: Uuid,
    pub connector_id: Uuid,
    pub repo: String,
    pub number: i64,
    pub author_kind: DraftAuthorKind,
    /// The owning user, when `author_kind` is `user`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_user_id: Option<Uuid>,
    /// The authoring session, when `author_kind` is `agent`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_session_id: Option<String>,
    pub verdict: ReviewVerdict,
    pub status: DraftStatus,
    pub created_at: String,
    pub updated_at: String,
    /// The draft's inline comments, oldest first.
    pub comments: Vec<DraftCommentInfo>,
}

// ---------------------------------------------------------------------------
// GH-VIEW-6: blob-keyed "reviewed" marks (docs §6.2).
//
// A reviewer marks a file reviewed keyed to its blob SHA (`DiffFile.blob_sha`).
// On a later push the diff reloads with fresh blob SHAs; the webui keeps a file
// "reviewed" only when its current blob SHA still matches the stored mark, so a
// push re-flags ONLY the files that actually changed. Marks are per user+PR,
// persisted in `github.viewed_marks`.
// ---------------------------------------------------------------------------

/// Request body for `POST .../{number}/mark-viewed` and `.../unmark-viewed`.
///
/// The file path plus the blob SHA the reviewer saw. The path is the per-file
/// identity; the blob SHA is the content the mark is keyed to.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MarkViewedRequest {
    /// Head-side file path being (un)marked.
    pub path: String,
    /// The blob SHA the reviewer saw for this path (from `DiffFile.blob_sha`).
    /// Required on mark; ignored on unmark (the path is the identity there).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob_sha: Option<String>,
}

/// One blob-keyed "reviewed" mark for a file in a PR, scoped to the caller.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ViewedMarkInfo {
    /// Head-side file path the mark applies to.
    pub path: String,
    /// The blob SHA the file had when marked reviewed. The webui treats a file
    /// as reviewed only while its current `DiffFile.blob_sha` equals this — a
    /// later push that changes the file rotates its blob SHA and re-flags it.
    pub blob_sha: String,
    pub marked_at: String,
}

/// One row in the `/github` PR inbox: the synced PR plus its derived attention
/// bucket and pre-aggregated CI/review summaries.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PullInboxItem {
    /// The connector that tracks this PR (multi-account scoping in the UI).
    pub connector_id: Uuid,
    /// `owner/name` slug.
    pub repo: String,
    /// PR number within the repo.
    pub number: i64,
    pub title: String,
    /// `open` | `closed`.
    pub state: String,
    pub merged: bool,
    pub draft: bool,
    pub author: String,
    pub head_ref: String,
    pub base_ref: String,
    /// GitHub's `mergeable_state`, when known.
    pub mergeable_state: Option<String>,
    /// GitHub's last-update timestamp (ISO-8601) — the inbox sorts on it.
    pub gh_updated_at: String,
    /// The most-actionable attention bucket (GH-CONN-6 derivation).
    pub bucket: AttentionBucket,
    /// Pre-aggregated CI state for the head SHA.
    pub checks: CheckSummary,
    /// Pre-aggregated review state.
    pub reviews: ReviewSummary,
}

// ---------------------------------------------------------------------------
// GH-VIEW-5: publish a review (batched) + pull-down of existing GitHub threads.
//
// Publishing resolves each draft comment's GH-VIEW-2 anchor against the *current*
// head SHA and submits ONE `POST /repos/{o}/{r}/pulls/{n}/reviews` with the
// batched comments + verdict — never per-comment spam. A comment whose anchor no
// longer resolves (line gone from the diff) is skipped and reported; a draft made
// against a stale head SHA (force-push) refuses to publish rather than mis-place.
// No credential or token is represented here.
// ---------------------------------------------------------------------------

/// Request body for `POST .../{number}/publish-review` — publish a draft as one
/// batched GitHub review. The draft (and its verdict) is named by `draft_id`; an
/// optional `summary` becomes the review body.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PublishReviewRequest {
    /// The open draft to publish.
    pub draft_id: Uuid,
    /// Optional review summary (the `body` of the submitted review).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// The head SHA the reviewer was viewing the diff against when they hit
    /// Publish. The server compares it to the PR's *current* head SHA; a mismatch
    /// means the PR was force-pushed/rebased since the draft was authored, so the
    /// publish is refused (rather than mis-placing comments onto rotated lines).
    /// `None` skips the guard (the caller accepts re-anchoring against current).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_head_sha: Option<String>,
}

/// One draft comment that could not be anchored at publish time, so it was left
/// out of the submitted review. The webui surfaces these so the reviewer knows a
/// comment did not post (rather than silently dropping it).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SkippedComment {
    /// The draft comment's id.
    pub comment_id: Uuid,
    /// The file path it was anchored on.
    pub path: String,
    /// 1-based line on its side.
    pub line: u32,
    /// Why it could not be anchored (the [`AnchorError`] reason).
    pub reason: AnchorError,
}

/// Outcome of a successful publish: the GitHub review id, how many comments were
/// submitted, and which draft comments were skipped (un-anchorable).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PublishReviewResult {
    /// GitHub's submitted review id.
    pub review_id: i64,
    /// How many comments were included in the batched submission.
    pub submitted: u32,
    /// Draft comments left out because their anchor no longer resolved.
    pub skipped: Vec<SkippedComment>,
}

/// One pulled-down GitHub review comment (the posted side, CONN-3
/// `github.review_comments`), rendered inline alongside local drafts.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ReviewThreadCommentInfo {
    /// GitHub's review-comment id.
    pub comment_id: i64,
    pub author: String,
    pub body: String,
    pub created_at: String,
}

/// One pulled-down GitHub review thread (CONN-3 `github.review_threads`) plus its
/// comments, anchored on a diff line. Distinct from a local draft: it is already
/// posted on GitHub. The webui renders it inline, visually separate from drafts.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ReviewThreadInfo {
    pub thread_node_id: String,
    /// Head-side path the thread is anchored on, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// `LEFT` | `RIGHT` diff side, when anchored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side: Option<String>,
    /// 1-based line on `side`, when anchored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<i64>,
    pub resolved: bool,
    /// The thread's comments, oldest first.
    pub comments: Vec<ReviewThreadCommentInfo>,
}
