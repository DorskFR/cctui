//! Account pools — the declared set of accounts a session may run on.
//!
//! A pool answers the question neither `auto_account` nor the gateway's
//! failover could: *which accounts are interchangeable for this work?* Before
//! it, both ranked every account the caller could reach, so a personal session
//! could be bound (or, mid-run, rebound) to a work credential with nothing
//! saying that was allowed and nothing recording that it happened.
//!
//! Two rules give the boundary its teeth:
//!
//!   * membership is a deliberate act — no account is ever in a pool because
//!     it happens to be reachable;
//!   * membership is re-checked on every read ([`usable_members`]), not just
//!     when the pool was edited, so revoking a share or clearing
//!     `pool_eligible` takes the account out of every election immediately.

use chrono::{DateTime, Utc};
use sqlx::PgExecutor;
use uuid::Uuid;

/// How a launch picks among the members of a pool.
pub const STRATEGY_HEADROOM: &str = "headroom";
pub const STRATEGY_ORDERED: &str = "ordered";

/// Whether `s` names a strategy the table accepts.
pub fn valid_strategy(s: &str) -> bool {
    matches!(s, STRATEGY_HEADROOM | STRATEGY_ORDERED)
}

/// One pool, without its members.
#[derive(Clone, Debug, sqlx::FromRow, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct AccountPool {
    #[ts(type = "string")]
    pub id: Uuid,
    #[ts(type = "string")]
    pub user_id: Uuid,
    pub name: String,
    /// `headroom` (most allocation left wins) or `ordered` (first member with
    /// room, by `position`).
    pub strategy: String,
    /// Whether a live session bound to this pool may be moved between members
    /// when its account is refused.
    pub failover: bool,
    #[ts(type = "string")]
    pub created_at: DateTime<Utc>,
}

/// A member as the API renders it: enough for the UI to explain why an account
/// is or is not currently electable, without a second round trip.
#[derive(Clone, Debug, sqlx::FromRow, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct AccountPoolMember {
    #[ts(type = "string")]
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

const COLS: &str = "id, user_id, name, strategy, failover, created_at";

/// Every pool `owner` owns, name order for a stable UI. `owner` scopes the
/// read the same way `get` does: `None` (the admin token, which has no user
/// identity of its own) lists every user's pools rather than nobody's.
pub async fn list_for_owner(
    exec: impl PgExecutor<'_>,
    owner: Option<Uuid>,
) -> Result<Vec<AccountPool>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM account_pools \
         WHERE $1::uuid IS NULL OR user_id = $1 ORDER BY lower(name)"
    )))
    .bind(owner)
    .fetch_all(exec)
    .await
}

/// One pool by id. `owner` scopes the read; `None` (the admin token) reads any.
pub async fn get(
    exec: impl PgExecutor<'_>,
    id: Uuid,
    owner: Option<Uuid>,
) -> Result<Option<AccountPool>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM account_pools WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)"
    )))
    .bind(id)
    .bind(owner)
    .fetch_optional(exec)
    .await
}

/// One pool by the name a spawn typed. Case-insensitive, matching the unique
/// index, so `Perso` and `perso` are the same pool and neither can be created
/// twice.
pub async fn by_name(
    exec: impl PgExecutor<'_>,
    user_id: Uuid,
    name: &str,
) -> Result<Option<AccountPool>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM account_pools WHERE user_id = $1 AND lower(name) = lower($2)"
    )))
    .bind(user_id)
    .bind(name)
    .fetch_optional(exec)
    .await
}

/// The members of `pool_id` as stored, in election order.
pub async fn members(
    exec: impl PgExecutor<'_>,
    pool_id: Uuid,
) -> Result<Vec<AccountPoolMember>, sqlx::Error> {
    sqlx::query_as(
        "SELECT m.account_id, a.name, m.position, \
                (a.user_id = p.user_id) AS owned, a.pool_eligible \
           FROM account_pool_members m \
           JOIN account_pools p ON p.id = m.pool_id \
           JOIN accounts a      ON a.id = m.account_id \
          WHERE m.pool_id = $1 \
          ORDER BY m.position, lower(a.name)",
    )
    .bind(pool_id)
    .fetch_all(exec)
    .await
}

pub async fn create(
    exec: impl PgExecutor<'_>,
    user_id: Uuid,
    name: &str,
    strategy: &str,
    failover: bool,
) -> Result<AccountPool, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "INSERT INTO account_pools (user_id, name, strategy, failover) \
         VALUES ($1, $2, $3, $4) RETURNING {COLS}"
    )))
    .bind(user_id)
    .bind(name)
    .bind(strategy)
    .bind(failover)
    .fetch_one(exec)
    .await
}

/// Patch a pool. Every field is optional; `None` leaves it as stored.
pub async fn update(
    exec: impl PgExecutor<'_>,
    id: Uuid,
    owner: Option<Uuid>,
    name: Option<&str>,
    strategy: Option<&str>,
    failover: Option<bool>,
) -> Result<Option<AccountPool>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE account_pools SET \
             name     = COALESCE($3, name), \
             strategy = COALESCE($4, strategy), \
             failover = COALESCE($5, failover), \
             updated_at = now() \
          WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2) \
          RETURNING {COLS}"
    )))
    .bind(id)
    .bind(owner)
    .bind(name)
    .bind(strategy)
    .bind(failover)
    .fetch_optional(exec)
    .await
}

pub async fn delete(
    exec: impl PgExecutor<'_>,
    id: Uuid,
    owner: Option<Uuid>,
) -> Result<bool, sqlx::Error> {
    let res = sqlx::query(
        "DELETE FROM account_pools WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)",
    )
    .bind(id)
    .bind(owner)
    .execute(exec)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// Replace the membership of `pool_id` with `account_ids`, in the given order.
///
/// Whole-list replacement rather than add/remove endpoints: order is part of
/// the meaning under the `ordered` strategy, and a caller reordering members
/// would otherwise have to guess how two concurrent edits interleave.
pub async fn set_members(
    pool: &sqlx::PgPool,
    pool_id: Uuid,
    account_ids: &[Uuid],
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM account_pool_members WHERE pool_id = $1")
        .bind(pool_id)
        .execute(&mut *tx)
        .await?;
    for (position, account_id) in account_ids.iter().enumerate() {
        sqlx::query(
            "INSERT INTO account_pool_members (pool_id, account_id, position) \
             VALUES ($1, $2, $3) ON CONFLICT (pool_id, account_id) DO NOTHING",
        )
        .bind(pool_id)
        .bind(account_id)
        .bind(i32::try_from(position).unwrap_or(i32::MAX))
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await
}

/// One member a launch or a failover may actually bind right now.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct UsableMember {
    pub account_id: Uuid,
    pub name: String,
    /// The credential in the requested family that will serve.
    pub provider_id: Uuid,
    pub soft_limits_json: Option<serde_json::Value>,
}

/// The members of `pool_id` that `user_id` may bind in `family`, in election
/// order.
///
/// Re-derives eligibility from the live grant state instead of trusting the
/// membership row: the account must still be the pool owner's own, or still be
/// shared with them *and* still carry the owner's `pool_eligible`. A member
/// that fails either test simply is not returned — the pool shrinks quietly to
/// what is genuinely allowed rather than erroring, and the UI shows why from
/// [`members`].
pub async fn usable_members(
    exec: impl PgExecutor<'_>,
    pool_id: Uuid,
    user_id: Uuid,
    family: &str,
) -> Result<Vec<UsableMember>, sqlx::Error> {
    sqlx::query_as(
        "SELECT a.id AS account_id, a.name, ap.id AS provider_id, ap.soft_limits_json \
           FROM account_pool_members m \
           JOIN accounts a          ON a.id = m.account_id \
           JOIN account_providers ap ON ap.account_id = a.id AND ap.family = $3 \
          WHERE m.pool_id = $1 \
            AND (a.user_id = $2 \
                 OR (a.pool_eligible AND EXISTS ( \
                     SELECT 1 FROM resource_shares s \
                      WHERE s.resource_type = 'account' AND s.resource_id = a.id \
                        AND s.grantee_id = $2 AND s.revoked_at IS NULL))) \
          ORDER BY m.position, lower(a.name)",
    )
    .bind(pool_id)
    .bind(user_id)
    .bind(family)
    .fetch_all(exec)
    .await
}

/// One recorded mid-session account move.
#[derive(Clone, Debug, sqlx::FromRow, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct SessionRebind {
    #[ts(type = "string")]
    pub id: Uuid,
    pub session_id: String,
    #[ts(type = "string | null")]
    pub pool_id: Option<Uuid>,
    pub from_account: String,
    pub to_account: String,
    /// `pool` or `redirect` — which mechanism moved the session.
    pub reason: String,
    #[ts(type = "string")]
    pub created_at: DateTime<Utc>,
}

/// Append a rebind to the session's history. Best-effort by contract: the
/// caller has already moved the session, and losing the audit row must not
/// turn a successful failover into a failure.
pub async fn record_rebind(
    exec: impl PgExecutor<'_>,
    session_id: &str,
    pool_id: Option<Uuid>,
    from_account: &str,
    to_account: &str,
    reason: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO session_account_rebinds \
           (session_id, pool_id, from_account, to_account, reason) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(session_id)
    .bind(pool_id)
    .bind(from_account)
    .bind(to_account)
    .bind(reason)
    .execute(exec)
    .await
    .map(|_| ())
}

/// A session's account moves, newest first.
pub async fn rebinds_for_session(
    exec: impl PgExecutor<'_>,
    session_id: &str,
) -> Result<Vec<SessionRebind>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, session_id, pool_id, from_account, to_account, reason, created_at \
           FROM session_account_rebinds WHERE session_id = $1 ORDER BY created_at DESC",
    )
    .bind(session_id)
    .fetch_all(exec)
    .await
}
