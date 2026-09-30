//! `/api/v1/account-pools` — pools, their membership and aggregated quota.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::api::pool_usage::PoolUsageWindow;

/// One pool, without its members.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AccountPool {
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub id: Uuid,
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub user_id: Uuid,
    pub name: String,
    /// `headroom` (most allocation left wins) or `ordered` (first member with
    /// room, by `position`).
    pub strategy: String,
    /// Whether a live session bound to this pool may be moved between members
    /// when its account is refused.
    pub failover: bool,
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub created_at: DateTime<Utc>,
}

/// A member as the API renders it: enough for the UI to explain why an account
/// is or is not currently electable, without a second round trip.
#[derive(Clone, Debug, serde::Serialize)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AccountPoolMember {
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub account_id: Uuid,
    pub name: String,
    pub position: i32,
    /// False when the member belongs to someone else (shared with the pool's
    /// owner). Such a member can leave the pool without warning: the owner may
    /// revoke the share or clear `pool_eligible`.
    pub owned: bool,
    /// The owner's veto. A shared member with this false is kept in the row
    /// (so the UI can say why it stopped counting) but never elected.
    pub pool_eligible: bool,
}

/// One recorded mid-session account move.
#[derive(Clone, Debug, serde::Serialize)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SessionRebind {
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub id: Uuid,
    pub session_id: String,
    #[cfg_attr(feature = "ts", ts(type = "string | null"))]
    pub pool_id: Option<Uuid>,
    pub from_account: String,
    pub to_account: String,
    /// `pool` or `redirect` — which mechanism moved the session.
    pub reason: String,
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub created_at: DateTime<Utc>,
}

/// A pool with its membership — what the accounts screen renders.
#[derive(serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AccountPoolView {
    #[serde(flatten)]
    pub pool: AccountPool,
    pub members: Vec<AccountPoolMember>,
}

/// A pool's quota, aggregated per provider family — what the pool zone and
/// the stats panel render. See [`crate::api::pool_usage`] for the arithmetic.
#[derive(serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PoolUsageView {
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub pool_id: Uuid,
    pub name: String,
    pub strategy: String,
    /// Off means a projection only speaks for launches: a live session stays
    /// on its member and hits that member's wall.
    pub failover: bool,
    pub families: Vec<PoolFamilyUsage>,
}

/// One provider family inside a pool.
///
/// Only its members are interchangeable (a claude-code spawn elects among the anthropic
/// credentials, a codex spawn among the openai ones), so only they are aggregated together.
#[derive(serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PoolFamilyUsage {
    /// `anthropic` | `openai` | `fireworks`.
    pub family: String,
    pub members: Vec<PoolUsageMember>,
    pub windows: Vec<PoolUsageWindow>,
}

#[derive(serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PoolUsageMember {
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub account_id: Uuid,
    pub name: String,
    pub emoji: Option<String>,
    /// `accounts.pool_weight`.
    pub weight: f64,
    /// False when the credential's usage could not be read: absent from the
    /// aggregate rather than counted as empty or as full.
    pub usage_known: bool,
}

#[derive(serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct CreatePoolRequest {
    pub name: String,
    /// `headroom` (default) or `ordered`.
    #[cfg_attr(feature = "ts", ts(optional))]
    pub strategy: Option<String>,
    /// Whether a live session may be moved between members. Defaults to false:
    /// creating a pool changes how launches pick, nothing about running work.
    #[cfg_attr(feature = "ts", ts(optional))]
    pub failover: Option<bool>,
    /// Members, in election order for the `ordered` strategy.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "string[]", optional))]
    pub accounts: Vec<Uuid>,
    /// The admin token has no user identity and must name the pool's owner.
    #[cfg_attr(feature = "ts", ts(type = "string | null", optional))]
    pub user_id: Option<Uuid>,
}

#[derive(serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct UpdatePoolRequest {
    #[cfg_attr(feature = "ts", ts(optional))]
    pub name: Option<String>,
    #[cfg_attr(feature = "ts", ts(optional))]
    pub strategy: Option<String>,
    #[cfg_attr(feature = "ts", ts(optional))]
    pub failover: Option<bool>,
    /// Absent leaves the membership alone; present replaces it wholesale, in
    /// the given order.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "string[] | null", optional))]
    pub accounts: Option<Vec<Uuid>>,
}
