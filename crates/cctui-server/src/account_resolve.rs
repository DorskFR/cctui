//! Resolve a caller-supplied name to the account a session binds.
//!
//! Every entry point that accepts an account name accepts a pool name too: if
//! the name is a pool, a member is elected *at bind time* by the pool's
//! strategy and the pool is returned so the caller can stamp it on the
//! session token. Without that stamp the pool is inert for the session —
//! `gateway::failover` keys in-pool rebinding off `session_tokens.pool_id`.
//!
//! Election reuses the ranker in [`crate::account_pick`]; there is exactly one
//! of those, and this module does not add a second.

use uuid::Uuid;

use crate::account_pick::{Pick, pick_account, pick_in_order};
use crate::routes::gateway::Family;
use crate::state::AppState;
use crate::store::account_pools::{self, AccountPool, STRATEGY_ORDERED};

/// What a name turned out to denote.
#[derive(Debug, PartialEq, Eq)]
pub enum Target {
    Account(String),
    Pool(AccountPool),
}

/// Why a name could not be bound.
#[derive(Debug)]
pub enum ResolveError {
    /// The request cannot be served as asked — a 400 with a caller-readable
    /// reason (which accounts were considered and why each was skipped).
    Rejected(String),
    Db,
}

/// The account to bind, plus the pool it must stay inside (if any).
#[derive(Debug, PartialEq, Eq)]
pub struct Bound {
    pub account: String,
    pub pool_id: Option<Uuid>,
}

/// An account and a pool may share a name. The account wins: an explicit
/// account name must never be silently redirected to a set of other
/// credentials, and every caller that names an account today keeps its exact
/// behaviour. A pool name only takes effect when no account answers to it.
pub fn choose_target(name: &str, account_exists: bool, pool: Option<AccountPool>) -> Target {
    match pool {
        Some(pool) if !account_exists => Target::Pool(pool),
        _ => Target::Account(name.to_owned()),
    }
}

/// Rank `candidates` by the pool's strategy and return the elected member.
///
/// Split from the DB work so the election is unit-testable: `Exhausted` is
/// only reached when every member was *readable* and out of allocation, so a
/// flaky usage endpoint leaves a member `usage_known: false` and the pool
/// still elects rather than wedging the caller's queue.
pub fn elect(
    pool_name: &str,
    strategy: &str,
    candidates: &[crate::account_pick::Candidate],
    model: Option<&str>,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<String, ResolveError> {
    let pick = if strategy == STRATEGY_ORDERED {
        pick_in_order(candidates, model, now)
    } else {
        pick_account(candidates, model, now)
    };
    match pick {
        Pick::Chosen { name, .. } => Ok(name),
        Pick::Exhausted(blocked) => {
            let detail = blocked
                .iter()
                .map(|b| format!("{}: {}", b.name, b.reason))
                .collect::<Vec<_>>()
                .join("; ");
            Err(ResolveError::Rejected(format!(
                "no account in pool {pool_name:?} has allocation left for this session ({detail})"
            )))
        }
        // `candidates` is non-empty at every call site, so the ranker cannot
        // return None; treat it as exhaustion rather than widening the search.
        Pick::None => Err(ResolveError::Rejected(format!(
            "no account in pool {pool_name:?} can serve this session"
        ))),
    }
}

/// Whether `user_id` can reach an account called `name` (their own, or one
/// shared with them) — the same reachability `mint` uses.
async fn account_exists(state: &AppState, user_id: Uuid, name: &str) -> Result<bool, sqlx::Error> {
    let found: Option<Uuid> = sqlx::query_scalar(
        "SELECT a.id FROM accounts a \
         WHERE a.name = $2 \
           AND (a.user_id = $1 OR EXISTS ( \
               SELECT 1 FROM resource_shares s \
                WHERE s.resource_type = 'account' AND s.resource_id = a.id \
                  AND s.grantee_id = $1 AND s.revoked_at IS NULL)) \
         LIMIT 1",
    )
    .bind(user_id)
    .bind(name)
    .fetch_optional(&state.pool)
    .await?;
    Ok(found.is_some())
}

/// Elect the member of `pool` that will serve this session.
///
/// Usage is measured through any redirect chain, so a member pointed elsewhere
/// by an explicit rule is scored on the credential that will actually serve.
/// The elected *name* stays the member's, so mint applies that rule as it
/// would for any launch.
pub async fn elect_pool_member(
    state: &AppState,
    user_id: Uuid,
    family: Family,
    model: Option<&str>,
    pool: &AccountPool,
) -> Result<String, ResolveError> {
    let members = account_pools::usable_members(&state.pool, pool.id, user_id, family.label())
        .await
        .map_err(|e| {
            tracing::error!("reading account pool members: {e}");
            ResolveError::Db
        })?;
    if members.is_empty() {
        // Deliberately not a silent fallback to the wider set: the caller
        // named a boundary, and quietly launching outside it is the exact
        // failure pools exist to prevent.
        return Err(ResolveError::Rejected(format!(
            "pool {:?} has no usable {} account — add one, or check that its members \
             are still shared with you",
            pool.name,
            family.label()
        )));
    }

    let rules = crate::store::account_redirects::live_for_launch(&state.pool, user_id)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!("pool account: reading redirects failed, ranking origins: {e}");
            Vec::new()
        });
    let providers: std::collections::HashMap<Uuid, Uuid> =
        members.iter().map(|m| (m.account_id, m.provider_id)).collect();
    let usages = futures_util::future::join_all(members.iter().map(|m| {
        let effective = crate::store::account_redirects::follow_account_chain(
            &rules,
            m.account_id,
            family.label(),
        )
        .and_then(|to| providers.get(&to).copied())
        .unwrap_or(m.provider_id);
        async move { crate::routes::gateway::usage_for_soft_limit(state, effective).await }
    }))
    .await;

    let candidates: Vec<crate::account_pick::Candidate> = members
        .iter()
        .zip(usages)
        .map(|(m, usage)| crate::account_pick::Candidate {
            name: m.name.clone(),
            windows: usage
                .as_ref()
                .map(crate::soft_limit::normalize_usage_windows)
                .unwrap_or_default(),
            limits: crate::soft_limit::SoftLimits::from_json(m.soft_limits_json.as_ref()),
            usage_known: usage.is_some(),
        })
        .collect();

    let name = elect(&pool.name, &pool.strategy, &candidates, model, chrono::Utc::now())?;
    tracing::info!(
        %user_id, account = %name, pool = %pool.name, strategy = %pool.strategy,
        family = %family.label(), "pool account: bound a member of the pool"
    );
    Ok(name)
}

/// The pool half of resolution only: `name` must name a pool.
pub async fn resolve_pool(
    state: &AppState,
    user_id: Uuid,
    family: Family,
    model: Option<&str>,
    pool_name: &str,
) -> Result<(String, Uuid), ResolveError> {
    let pool = account_pools::by_name(&state.pool, user_id, pool_name)
        .await
        .map_err(|e| {
            tracing::error!("resolving account pool: {e}");
            ResolveError::Db
        })?
        .ok_or_else(|| ResolveError::Rejected(format!("no account pool named {pool_name:?}")))?;
    let account = elect_pool_member(state, user_id, family, model, &pool).await?;
    Ok((account, pool.id))
}

/// Elect within a pool already identified by id — the shape a stored binding
/// (`dispatchers.default_pool_id`) has, where no name is involved and so no
/// account can shadow it.
pub async fn resolve_pool_by_id(
    state: &AppState,
    user_id: Uuid,
    family: Family,
    model: Option<&str>,
    pool_id: Uuid,
) -> Result<(String, Uuid), ResolveError> {
    let pool = account_pools::get(&state.pool, pool_id, Some(user_id))
        .await
        .map_err(|e| {
            tracing::error!("resolving account pool: {e}");
            ResolveError::Db
        })?
        .ok_or_else(|| ResolveError::Rejected("the bound account pool no longer exists".into()))?;
    let account = elect_pool_member(state, user_id, family, model, &pool).await?;
    Ok((account, pool.id))
}

/// Resolve a name that may be either an account or a pool.
pub async fn resolve_account_or_pool(
    state: &AppState,
    user_id: Uuid,
    family: Family,
    model: Option<&str>,
    name: &str,
) -> Result<Bound, ResolveError> {
    let pool = account_pools::by_name(&state.pool, user_id, name).await.map_err(|e| {
        tracing::error!("resolving account pool: {e}");
        ResolveError::Db
    })?;
    let Some(pool) = pool else { return Ok(Bound { account: name.to_owned(), pool_id: None }) };
    let exists = account_exists(state, user_id, name).await.map_err(|e| {
        tracing::error!("resolving account by name: {e}");
        ResolveError::Db
    })?;
    match choose_target(name, exists, Some(pool)) {
        Target::Account(a) => Ok(Bound { account: a, pool_id: None }),
        Target::Pool(pool) => {
            let account = elect_pool_member(state, user_id, family, model, &pool).await?;
            Ok(Bound { account, pool_id: Some(pool.id) })
        }
    }
}

/// Record the pool a live session may be moved inside. Best-effort: the
/// session is already provisioned on a member, and losing the stamp only costs
/// it the right to be rebound later.
pub async fn stamp_pool(state: &AppState, session_id: &str, pool_id: Uuid) {
    if let Err(e) = sqlx::query(
        "UPDATE session_tokens SET pool_id = $2 WHERE session_id = $1 AND revoked_at IS NULL",
    )
    .bind(session_id)
    .bind(pool_id)
    .execute(&state.pool)
    .await
    {
        tracing::warn!(%session_id, error = %e, "could not stamp the session's account pool");
    }
}

#[cfg(test)]
mod tests {
    use super::{ResolveError, Target, choose_target, elect};
    use crate::account_pick::Candidate;
    use crate::soft_limit::SoftLimits;
    use crate::store::account_pools::{AccountPool, STRATEGY_HEADROOM, STRATEGY_ORDERED};

    fn pool(name: &str, strategy: &str) -> AccountPool {
        AccountPool {
            id: uuid::Uuid::nil(),
            user_id: uuid::Uuid::nil(),
            name: name.to_owned(),
            strategy: strategy.to_owned(),
            failover: false,
            created_at: chrono::Utc::now(),
        }
    }

    fn candidate(name: &str, five_hour_pct: f64) -> Candidate {
        Candidate {
            name: name.to_owned(),
            windows: vec![crate::soft_limit::UsageWindow {
                key: crate::soft_limit::KEY_SESSION.to_owned(),
                kind: "session".to_owned(),
                label: "5h".to_owned(),
                utilization: five_hour_pct,
                amount_usd: None,
                resets_at: Some(chrono::Utc::now() + chrono::Duration::hours(3)),
                model_id: None,
                model_display_name: None,
            }],
            limits: SoftLimits::default(),
            usage_known: true,
        }
    }

    #[test]
    fn an_account_wins_a_name_shared_with_a_pool() {
        assert_eq!(
            choose_target("work", true, Some(pool("work", STRATEGY_HEADROOM))),
            Target::Account("work".to_owned())
        );
    }

    #[test]
    fn a_pool_name_no_account_answers_to_resolves_to_the_pool() {
        let p = pool("work", STRATEGY_HEADROOM);
        assert_eq!(choose_target("work", false, Some(p.clone())), Target::Pool(p));
    }

    #[test]
    fn an_unknown_name_stays_an_account_name() {
        // The account paths already own the "no such account" error; resolution
        // must not invent a different one.
        assert_eq!(choose_target("nope", false, None), Target::Account("nope".to_owned()));
    }

    #[test]
    fn election_skips_the_exhausted_member() {
        let candidates = [candidate("hirobot", 100.0), candidate("pafin", 9.0)];
        assert_eq!(
            elect("work", STRATEGY_HEADROOM, &candidates, None, chrono::Utc::now()).unwrap(),
            "pafin"
        );
    }

    #[test]
    fn ordered_election_skips_an_exhausted_first_member() {
        let candidates = [candidate("hirobot", 100.0), candidate("pafin", 9.0)];
        assert_eq!(
            elect("work", STRATEGY_ORDERED, &candidates, None, chrono::Utc::now()).unwrap(),
            "pafin"
        );
    }

    #[test]
    fn every_member_out_is_rejected_and_names_them() {
        let candidates = [candidate("hirobot", 100.0), candidate("pafin", 100.0)];
        let err = elect("work", STRATEGY_HEADROOM, &candidates, None, chrono::Utc::now())
            .expect_err("all members exhausted");
        let ResolveError::Rejected(msg) = err else { panic!("expected a rejection") };
        assert!(msg.contains("hirobot"), "{msg}");
        assert!(msg.contains("pafin"), "{msg}");
    }

    #[test]
    fn an_unreadable_member_still_elects() {
        // A flaky usage endpoint must not wedge a dispatch queue.
        let mut unknown = candidate("pafin", 0.0);
        unknown.usage_known = false;
        unknown.windows.clear();
        let candidates = [candidate("hirobot", 100.0), unknown];
        assert_eq!(
            elect("work", STRATEGY_HEADROOM, &candidates, None, chrono::Utc::now()).unwrap(),
            "pafin"
        );
    }
}
