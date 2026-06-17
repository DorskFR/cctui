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
