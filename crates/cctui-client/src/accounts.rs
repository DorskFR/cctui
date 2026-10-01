//! Wire shapes for `/accounts`, `/account-pools` and `/redirects`.
//!
//! The proto views are `Serialize`-only, so these mirror the fields the TUI
//! reads. Ids stay `String`: the TUI never computes with them.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// One provider credential on an account.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AccountProvider {
    pub id: String,
    pub provider: String,
    pub family: String,
    pub managed: bool,
    pub needs_reauth: bool,
    #[serde(default)]
    pub last_auth_error: Option<String>,
    #[serde(default)]
    pub est_cost_usd: f64,
    #[serde(default)]
    pub total_tokens: i64,
    #[serde(default)]
    pub last_used_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub header_pin: bool,
}

/// One account identity, as `GET /accounts` reports it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Account {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub emoji: Option<String>,
    pub user_id: String,
    #[serde(default)]
    pub user_name: Option<String>,
    #[serde(default)]
    pub providers: Vec<AccountProvider>,
    pub pool_eligible: bool,
    pub pool_weight: f32,
}

impl Account {
    /// Provider ids, in the order the server returned them.
    #[must_use]
    pub fn provider_names(&self) -> Vec<&str> {
        self.providers.iter().map(|p| p.provider.as_str()).collect()
    }
}

/// `PATCH /accounts/{id}`. An absent field is left alone server-side.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct UpdateAccount {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emoji: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pool_eligible: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pool_weight: Option<f32>,
}

/// One pool with its membership (`AccountPoolView`, whose pool is flattened).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AccountPool {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub strategy: String,
    pub failover: bool,
    #[serde(default)]
    pub members: Vec<AccountPoolMember>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AccountPoolMember {
    pub account_id: String,
    pub name: String,
    pub position: i32,
    pub owned: bool,
    pub pool_eligible: bool,
}

/// `POST /account-pools`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CreatePool {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strategy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failover: Option<bool>,
    pub accounts: Vec<String>,
}

/// `PATCH /account-pools/{id}`. `accounts` present replaces the membership
/// wholesale, in the given order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct UpdatePool {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strategy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failover: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accounts: Option<Vec<String>>,
}

/// One live redirect rule, as `GET /redirects` reports it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AccountRedirect {
    pub id: String,
    pub from_account: String,
    #[serde(default)]
    pub to_account: Option<String>,
    pub family: String,
    #[serde(default)]
    pub to_model: Option<String>,
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub reason: Option<String>,
}

/// `PUT /accounts/{id}/redirect`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PutRedirect {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_account: Option<String>,
    pub family: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// `POST /accounts/{id}/limit-reset`, where `{id}` is a provider-row id.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct LimitResetRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credit_id: Option<String>,
}

/// What a claimed reset answers.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct LimitResetOutcome {
    pub provider: String,
    /// Upstream's own word, `error` when the call failed, `unconfirmed` when it
    /// succeeded with an unreadable body.
    pub outcome: String,
    #[serde(default)]
    pub credit_id: Option<String>,
    #[serde(default)]
    pub next_available_at: Option<String>,
    #[serde(default)]
    pub weekly_resets_at: Option<String>,
    /// The claim matched a prior attempt and nothing new was sent upstream.
    #[serde(default)]
    pub reused: bool,
}

impl LimitResetOutcome {
    /// Only `reset` means a window actually moved; `unconfirmed` means the call
    /// went through with an unreadable body and must not read as a rejection.
    #[must_use]
    pub fn reset(&self) -> bool {
        self.outcome == "reset"
    }
}
