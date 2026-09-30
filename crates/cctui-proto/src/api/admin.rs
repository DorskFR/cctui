//! `/api/v1/admin/*` and `/api/v1/users/*` — users, machines, keys and tokens.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
#[cfg(feature = "ts")]
use ts_rs::TS;
use uuid::Uuid;

#[derive(Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct CreateUserRequest {
    pub name: String,
}

#[derive(Serialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct CreateUserResponse {
    pub id: Uuid,
    pub name: String,
    pub key: String,
}

#[derive(Serialize)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct UserRow {
    pub id: Uuid,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    /// Temporary off switch — auth fails while set, nothing is
    /// invalidated, clearing restores. Distinct from the permanent revoke.
    pub disabled_at: Option<DateTime<Utc>>,
    /// Per-user dispatch permission. Enforced on `POST
    /// /sessions/dispatch`; defaults TRUE.
    pub can_dispatch: bool,
    /// Latest use of any of the user's keys; None until one authenticates.
    #[cfg_attr(feature = "sqlx", sqlx(default))]
    pub last_seen_at: Option<DateTime<Utc>>,
}

#[derive(Serialize)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct MachineRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub display_name: Option<String>,
    pub first_seen_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    /// `persistent` (a real daemon) or `ephemeral` (a dispatch/worker pod).
    /// The New-session picker hides `ephemeral` machines.
    pub kind: String,
    /// Operator-set badge hue (0-359). `None` = hash of the name.
    pub hue: Option<i16>,
    /// Non-secret machine-key fragment, e.g. `cctui_m_ab1234…ef34`.
    /// `None` for machines enrolled before the preview column existed.
    pub key_preview: Option<String>,
    /// Derived online/stale/offline tier from `last_seen_at` age.
    /// Not a DB column — `#[sqlx(skip)]` makes `query_as` ignore it (filled via
    /// `Default`); the handler fills it in from `last_seen_at` after the fetch.
    #[cfg_attr(feature = "sqlx", sqlx(skip))]
    pub liveness: crate::models::MachineLiveness,
}

#[derive(Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct RenameMachineRequest {
    /// `None` clears the override so the UI falls back to `name`.
    pub display_name: Option<String>,
    /// Badge hue override (0-359). `None` clears it (hash fallback).
    /// The PATCH replaces both fields, so callers send the full pair.
    #[serde(default)]
    pub hue: Option<i16>,
}

/// Partial update of a user. Any field left `None` is unchanged, so
/// the same endpoint serves both rename and the dispatch-permission toggle.
#[derive(Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct UpdateUserRequest {
    /// Blank/whitespace is rejected (name is `NOT NULL`); `None` leaves it.
    pub name: Option<String>,
    pub can_dispatch: Option<bool>,
    /// `true` sets `disabled_at = now()`, `false` clears it.
    pub disabled: Option<bool>,
}

#[derive(Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct RelabelTokenRequest {
    /// `None`/blank clears the label.
    pub label: Option<String>,
}

#[derive(Serialize)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct UserTokenRow {
    pub id: Uuid,
    pub label: Option<String>,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    /// Non-secret fragment for display, e.g. `cctui_u_ab12…ef34`.
    /// `None` for tokens minted before the preview column existed.
    pub token_preview: Option<String>,
}

#[derive(Serialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct RotateResponse {
    pub id: Uuid,
    pub key: String,
}

#[derive(Serialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct UserAclsResponse {
    pub user_id: Uuid,
    /// The user's ceiling (what its keys may be granted), as scope strings.
    pub scopes: Vec<String>,
}

#[derive(Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct SetAclsRequest {
    /// The full desired scope set (replaces the existing rows). Strings from the
    /// `read|dispatch|enroll|admin` set; unknown values are rejected.
    pub scopes: Vec<String>,
}

#[derive(Serialize)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct ApiKeyRow {
    pub id: Uuid,
    pub label: Option<String>,
    pub key_preview: Option<String>,
    pub kind: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    /// The key's granted scopes (`key_acls`), filled by the handler.
    #[cfg_attr(feature = "sqlx", sqlx(skip))]
    pub scopes: Vec<String>,
}

#[derive(Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct MintKeyRequest {
    pub label: Option<String>,
    /// Scopes to grant — must be ⊆ the owner's ceiling (enforced server-side).
    pub scopes: Vec<String>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Serialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct MintKeyResponse {
    pub id: Uuid,
    /// The plaintext token — returned ONCE, never recoverable after.
    pub key: String,
    pub scopes: Vec<String>,
}

#[derive(Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct MintTokenRequest {
    pub label: Option<String>,
    pub expires_at: Option<chrono::DateTime<Utc>>,
}

#[derive(serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct MintTokenResponse {
    pub token: String,
    pub label: Option<String>,
    pub expires_at: Option<chrono::DateTime<Utc>>,
}

