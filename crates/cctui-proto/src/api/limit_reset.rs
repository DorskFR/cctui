//! `/api/v1/accounts/{id}/limit-reset` — normalized usage-limit resets.

use uuid::Uuid;

/// What the account's latest usage payload says about a limit reset, normalized
/// across providers for the button in the usage row.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct LimitResetStatus {
    /// `codex` (reset credits) or `claude` (`cedar_ember` / `juniper_tide`).
    #[cfg_attr(feature = "ts", ts(type = "\"codex\" | \"claude\""))]
    pub kind: &'static str,
    /// Whether a claim would do anything right now.
    pub available: bool,
    /// Codex: the redeemable credit's title (e.g. "Full reset (Weekly + 5 hr)").
    /// Claude: the `cedar_ember` grant's label.
    pub title: Option<String>,
    /// Codex: the credit a claim would name. Claude: the `cedar_ember` grant id.
    pub credit_id: Option<String>,
    /// Claude: why the reset cannot be claimed (e.g. `not_at_wall`).
    pub ineligible_reason: Option<String>,
    /// Codex: the credit's expiry. Claude: a grant's `ends_at`, else when the
    /// at-wall reset comes back.
    pub next_available_at: Option<String>,
    pub weekly_resets_at: Option<String>,
    /// Claude `cedar_ember`: claims left on the named grant.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(type = "number | null", optional))]
    pub resets_left: Option<i64>,
    /// Claude `cedar_ember`: the grant may only be spent at a limit.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub requires_limit: Option<bool>,
    /// Claude `cedar_ember`: the limit windows a claim refills.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub clears: Option<Vec<String>>,
}

/// One reset a provider currently offers, normalized across Codex credits and
/// Claude's two programs. [`LimitResetStatus`] is the one the card button spends;
/// this is every offer the cached payload names.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct LimitResetEntry {
    /// `codex` (reset credit) or `claude` (`cedar_ember` grant / `juniper_tide`).
    #[cfg_attr(feature = "ts", ts(type = "\"codex\" | \"claude\""))]
    pub kind: &'static str,
    /// What a claim names: a Codex credit id, a `cedar_ember` grant id, or the
    /// program name for `juniper_tide`.
    pub id: String,
    /// Codex `title` / Claude `label`; absent when upstream named none.
    pub title: Option<String>,
    /// The limit windows a claim refills (`five_hour`, `seven_day`, …). Empty
    /// when upstream did not say, which the UI must not read as "nothing".
    pub restores: Vec<String>,
    /// Codex `expires_at` / Claude `ends_at`, or when the at-wall program comes
    /// back.
    pub expires_at: Option<String>,
    /// Claude `cedar_ember`: claims left on this grant.
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub resets_left: Option<i64>,
    /// Claude `cedar_ember`: the grant may only be spent at a limit.
    pub requires_limit: Option<bool>,
    /// Whether claiming this entry right now would do anything.
    pub usable: bool,
    /// Why not, when `usable` is false.
    pub unusable_reason: Option<String>,
}

#[derive(Debug, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct LimitResetResponse {
    pub account_id: Uuid,
    pub provider: String,
    /// Upstream outcome verbatim (`snake_case`), `error` when the call failed, or
    /// `unconfirmed` when it succeeded with a body we could not read.
    pub outcome: String,
    pub credit_id: Option<String>,
    pub next_available_at: Option<String>,
    pub weekly_resets_at: Option<String>,
    pub idempotency_key: String,
    /// The click matched a prior attempt and no new consume request was sent.
    pub reused: bool,
}

