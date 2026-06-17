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
