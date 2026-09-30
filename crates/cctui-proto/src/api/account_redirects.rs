//! `/api/v1/account-redirects` — temporary account/model redirect rules.

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// One live redirect rule. Exactly one of `to_account` / `to_model` is set
/// (enforced by `account_redirects_one_target`): a rule either moves new
/// sessions to another account or flips the model they spawn with — never both.
#[derive(Clone, Debug, serde::Serialize)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AccountRedirect {
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub id: Uuid,
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub user_id: Uuid,
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub from_account: Uuid,
    #[cfg_attr(feature = "ts", ts(type = "string | null"))]
    pub to_account: Option<Uuid>,
    pub family: String,
    pub match_model: Option<String>,
    pub to_model: Option<String>,
    #[cfg_attr(feature = "ts", ts(type = "string | null"))]
    pub expires_at: Option<DateTime<Utc>>,
    pub reason: Option<String>,
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub created_at: DateTime<Utc>,
}

#[derive(serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PutRedirectRequest {
    #[cfg_attr(feature = "ts", ts(type = "string | null", optional))]
    pub to_account: Option<Uuid>,
    #[cfg_attr(feature = "ts", ts(optional))]
    pub to_model: Option<String>,
    pub family: String,
    #[cfg_attr(feature = "ts", ts(optional))]
    pub match_model: Option<String>,
    #[cfg_attr(feature = "ts", ts(type = "string | null", optional))]
    pub until: Option<DateTime<Utc>>,
    #[cfg_attr(feature = "ts", ts(optional))]
    pub reason: Option<String>,
    /// The admin token has no user identity and must name the rule's owner.
    #[cfg_attr(feature = "ts", ts(type = "string | null", optional))]
    pub user_id: Option<Uuid>,
}

