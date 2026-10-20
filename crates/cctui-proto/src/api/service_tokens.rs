//! `/api/v1/accounts/{id}/service-tokens` — long-lived gateway credentials an
//! account owner mints for non-agent services. The plaintext is returned once.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
#[cfg(feature = "ts")]
use ts_rs::TS;
use uuid::Uuid;

/// One service token of an account, without its secret.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct ServiceTokenRow {
    pub id: Uuid,
    pub label: String,
    /// The `account_providers` row the token resolves to.
    pub provider_id: Uuid,
    pub provider: String,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

/// `POST /api/v1/accounts/{id}/service-tokens` payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct MintServiceTokenRequest {
    pub label: String,
    /// One of the account's `account_providers` ids.
    pub provider_id: Uuid,
}

/// The minted token. `token` is shown exactly once; only its hash is stored.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct MintServiceTokenResponse {
    pub token: String,
    pub row: ServiceTokenRow,
}
