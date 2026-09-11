//! `/api/v1/account-pools` — the named sets of interchangeable accounts.
//!
//! A pool is policy, and it is durable: "these accounts are interchangeable
//! for my work, pick among them and never leave". That is deliberately not the
//! same object as a redirect ([`super::account_redirects`]), which is an
//! incident — dated, one-off, "A is spent until 21:00, send new sessions to B".
//! Conflating the two is what made account movement feel arbitrary: one knob
//! meant both "balance my load forever" and "route around today's outage".
//!
//! Membership is checked twice. Here, when the pool is edited, so a caller
//! gets a clear 400 for an account they may not use; and again at every
//! election ([`crate::store::account_pools::usable_members`]) against live
//! grants, so a revoked share or a cleared `pool_eligible` takes effect
//! immediately rather than at the next edit.

use std::collections::BTreeMap;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use uuid::Uuid;

use super::accounts::{err, require_human, resolve_owner};
use crate::auth::AuthContext;
use crate::pool_usage::{self, MemberWindow, PoolUsageWindow, WindowIdentity};
use crate::routes::gateway;
use crate::state::AppState;
use crate::store::account_pools::{self, AccountPool, AccountPoolMember, SessionRebind};
use crate::store::usage_samples;

type ApiErr = (StatusCode, Json<serde_json::Value>);

/// A pool with its membership — what the accounts screen renders.
#[derive(serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct AccountPoolView {
    #[serde(flatten)]
    pub pool: AccountPool,
    pub members: Vec<AccountPoolMember>,
}

/// A pool's quota, aggregated per provider family — what the pool zone and
/// the stats panel render. See [`crate::pool_usage`] for the arithmetic.
#[derive(serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct PoolUsageView {
    #[ts(type = "string")]
    pub pool_id: Uuid,
    pub name: String,
    pub strategy: String,
    /// Off means a projection only speaks for launches: a live session stays
    /// on its member and hits that member's wall.
    pub failover: bool,
    pub families: Vec<PoolFamilyUsage>,
}

/// One provider family inside a pool: only its members are interchangeable
/// (a claude-code spawn elects among the anthropic credentials, a codex spawn
/// among the openai ones), so only they are aggregated together.
#[derive(serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct PoolFamilyUsage {
    /// `anthropic` | `openai` | `fireworks`.
    pub family: String,
    pub members: Vec<PoolUsageMember>,
    pub windows: Vec<PoolUsageWindow>,
}

#[derive(serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct PoolUsageMember {
    #[ts(type = "string")]
    pub account_id: Uuid,
    pub name: String,
    pub emoji: Option<String>,
    /// `accounts.pool_weight`.
    pub weight: f64,
    /// False when the credential's usage could not be read: absent from the
    /// aggregate rather than counted as empty or as full.
    pub usage_known: bool,
}

/// A pool member's credential in one family, with the account fields the
/// aggregate needs.
#[derive(sqlx::FromRow)]
struct MemberProviderRow {
    provider_id: Uuid,
    account_id: Uuid,
    family: String,
    name: String,
    emoji: Option<String>,
    pool_weight: f32,
}

/// `GET /account-pools/usage` — every pool of the caller with its members'
/// quota windows aggregated per family. Usage comes from the same per-provider
/// cache as `GET /accounts/usage`, so reading both costs one upstream fetch.
pub async fn pools_usage(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<Vec<PoolUsageView>>, ApiErr> {
    require_human(&ctx)?;
    let pools =
        account_pools::list_for_owner(&state.pool, ctx.owner_filter()).await.map_err(|e| {
            tracing::error!("listing account pools: {e}");
            err(StatusCode::INTERNAL_SERVER_ERROR, "could not list pools")
        })?;
    let mut views = Vec::with_capacity(pools.len());
    for pool in pools {
        let members = account_pools::members(&state.pool, pool.id).await.map_err(|e| {
            tracing::error!("listing pool members: {e}");
            err(StatusCode::INTERNAL_SERVER_ERROR, "could not list pool members")
        })?;
        // A vetoed member is never elected, so it must not count either.
        let ids: Vec<Uuid> =
            members.iter().filter(|m| m.pool_eligible).map(|m| m.account_id).collect();
        let rows: Vec<MemberProviderRow> = sqlx::query_as(
            "SELECT p.id AS provider_id, p.account_id, p.family, a.name, a.emoji, a.pool_weight \
               FROM account_providers p JOIN accounts a ON a.id = p.account_id \
              WHERE p.account_id = ANY($1) \
                AND p.provider IN ('anthropic', 'openai', 'fireworks') \
              ORDER BY p.family, lower(a.name)",
        )
        .bind(&ids)
        .fetch_all(&state.pool)
        .await
        .map_err(|e| {
            tracing::error!("listing pool member providers: {e}");
            err(StatusCode::INTERNAL_SERVER_ERROR, "could not read pool members")
        })?;
        let families = aggregate_families(&state, rows).await;
        views.push(PoolUsageView {
            pool_id: pool.id,
            name: pool.name,
            strategy: pool.strategy,
            failover: pool.failover,
            families,
        });
    }
    Ok(Json(views))
}

/// A family's members and, per window key, the identity of the window plus
/// every member's reading of it.
type FamilyReadings = (Vec<PoolUsageMember>, BTreeMap<String, (WindowIdentity, Vec<MemberWindow>)>);

/// Fetch each credential's usage and fold the readings into one aggregate per
/// family and window.
async fn aggregate_families(
    state: &AppState,
    rows: Vec<MemberProviderRow>,
) -> Vec<PoolFamilyUsage> {
    let now = chrono::Utc::now();
    let usages = futures_util::future::join_all(
        rows.iter().map(|r| gateway::usage_for_soft_limit(state, r.provider_id)),
    )
    .await;

    // family → (members, window key → (identity, readings)); BTreeMap keeps
    // families and windows in a stable order for the UI.
    let mut families: BTreeMap<String, FamilyReadings> = BTreeMap::default();
    for (row, usage) in rows.into_iter().zip(usages) {
        let windows: Vec<crate::soft_limit::UsageWindow> = usage
            .as_ref()
            .map(crate::soft_limit::normalize_usage_windows)
            .unwrap_or_default()
            .into_iter()
            .filter(|w| w.kind != "usd" && !w.key.starts_with("usd_"))
            .collect();
        let previous = match usage_samples::previous_for(
            &state.pool,
            row.provider_id,
            &windows,
            now,
        )
        .await
        {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!(provider_id = %row.provider_id, error = %e, "reading usage samples failed");
                Vec::new()
            }
        };
        let entry = families.entry(row.family.clone()).or_default();
        entry.0.push(PoolUsageMember {
            account_id: row.account_id,
            name: row.name,
            emoji: row.emoji,
            weight: f64::from(row.pool_weight),
            usage_known: usage.is_some(),
        });
        for w in windows {
            let prev = previous
                .iter()
                .find(|p| p.window_key == w.key)
                .map(usage_samples::PreviousSample::sample);
            let slot = entry.1.entry(w.key.clone()).or_insert_with(|| {
                (
                    WindowIdentity {
                        key: w.key.clone(),
                        kind: w.kind.clone(),
                        label: w.label.clone(),
                        model_display_name: w.model_display_name.clone(),
                    },
                    Vec::new(),
                )
            });
            slot.1.push(MemberWindow {
                account_id: row.account_id,
                weight: f64::from(row.pool_weight),
                utilization: w.utilization,
                resets_at: w.resets_at,
                duration: crate::pace::window_duration(&w.key),
                previous: prev,
            });
        }
    }

    families
        .into_iter()
        .map(|(family, (members, windows))| {
            let mut windows: Vec<PoolUsageWindow> = windows
                .into_values()
                .map(|(id, readings)| pool_usage::aggregate_window(id, &readings, now))
                .collect();
            windows.sort_by_key(|w| window_rank(&w.key));
            PoolFamilyUsage { family, members, windows }
        })
        .collect()
}

/// Display order: the session window, then the all-models week, then the
/// model-scoped weeks — the order the account cards use.
fn window_rank(key: &str) -> (u8, String) {
    let rank = match key {
        crate::soft_limit::KEY_SESSION => 0,
        crate::soft_limit::KEY_WEEKLY_ALL => 1,
        _ => 2,
    };
    (rank, key.to_owned())
}

#[derive(serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct CreatePoolRequest {
    pub name: String,
    /// `headroom` (default) or `ordered`.
    #[ts(optional)]
    pub strategy: Option<String>,
    /// Whether a live session may be moved between members. Defaults to false:
    /// creating a pool changes how launches pick, nothing about running work.
    #[ts(optional)]
    pub failover: Option<bool>,
    /// Members, in election order for the `ordered` strategy.
    #[serde(default)]
    #[ts(type = "string[]", optional)]
    pub accounts: Vec<Uuid>,
    /// The admin token has no user identity and must name the pool's owner.
    #[ts(type = "string | null", optional)]
    pub user_id: Option<Uuid>,
}

#[derive(serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct UpdatePoolRequest {
    #[ts(optional)]
    pub name: Option<String>,
    #[ts(optional)]
    pub strategy: Option<String>,
    #[ts(optional)]
    pub failover: Option<bool>,
    /// Absent leaves the membership alone; present replaces it wholesale, in
    /// the given order.
    #[serde(default)]
    #[ts(type = "string[] | null", optional)]
    pub accounts: Option<Vec<Uuid>>,
}

/// `GET /account-pools` — the caller's pools with their members. An admin
/// token has no pools of its own, so it reads every user's (`owner_filter`),
/// which is what the other three handlers already do.
pub async fn list_pools(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<Vec<AccountPoolView>>, ApiErr> {
    require_human(&ctx)?;
    let pools =
        account_pools::list_for_owner(&state.pool, ctx.owner_filter()).await.map_err(|e| {
            tracing::error!("listing account pools: {e}");
            err(StatusCode::INTERNAL_SERVER_ERROR, "could not list pools")
        })?;
    let mut views = Vec::with_capacity(pools.len());
    for pool in pools {
        let members = account_pools::members(&state.pool, pool.id).await.map_err(|e| {
            tracing::error!("listing pool members: {e}");
            err(StatusCode::INTERNAL_SERVER_ERROR, "could not list pool members")
        })?;
        views.push(AccountPoolView { pool, members });
    }
    Ok(Json(views))
}

/// `POST /account-pools` — create a pool and set its membership.
pub async fn create_pool(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<CreatePoolRequest>,
) -> Result<(StatusCode, Json<AccountPoolView>), ApiErr> {
    require_human(&ctx)?;
    let owner = resolve_owner(&ctx, req.user_id)?;
    let name = req.name.trim();
    if name.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "name required"));
    }
    let strategy = req.strategy.as_deref().unwrap_or(account_pools::STRATEGY_HEADROOM);
    if !account_pools::valid_strategy(strategy) {
        return Err(err(StatusCode::BAD_REQUEST, "strategy must be 'headroom' or 'ordered'"));
    }
    let accounts = dedup(&req.accounts);
    check_members(&state, owner, &accounts).await?;

    let pool =
        account_pools::create(&state.pool, owner, name, strategy, req.failover.unwrap_or(false))
            .await
            .map_err(|e| match &e {
                sqlx::Error::Database(db) if db.is_unique_violation() => {
                    err(StatusCode::CONFLICT, "a pool with that name already exists")
                }
                _ => {
                    tracing::error!("creating account pool: {e}");
                    err(StatusCode::INTERNAL_SERVER_ERROR, "could not create pool")
                }
            })?;
    account_pools::set_members(&state.pool, pool.id, &accounts).await.map_err(|e| {
        tracing::error!("setting pool members: {e}");
        err(StatusCode::INTERNAL_SERVER_ERROR, "could not set pool members")
    })?;
    let members = account_pools::members(&state.pool, pool.id).await.unwrap_or_default();
    Ok((StatusCode::CREATED, Json(AccountPoolView { pool, members })))
}

/// `PATCH /account-pools/{id}` — rename, restrategise, arm/disarm failover, or
/// replace the membership.
pub async fn update_pool(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdatePoolRequest>,
) -> Result<Json<AccountPoolView>, ApiErr> {
    require_human(&ctx)?;
    let owner = ctx.owner_filter();
    let name = req.name.as_deref().map(str::trim);
    if name == Some("") {
        return Err(err(StatusCode::BAD_REQUEST, "name required"));
    }
    if let Some(strategy) = req.strategy.as_deref()
        && !account_pools::valid_strategy(strategy)
    {
        return Err(err(StatusCode::BAD_REQUEST, "strategy must be 'headroom' or 'ordered'"));
    }
    let existing = account_pools::get(&state.pool, id, owner)
        .await
        .map_err(|e| {
            tracing::error!("reading account pool: {e}");
            err(StatusCode::INTERNAL_SERVER_ERROR, "could not read pool")
        })?
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "no such pool"))?;

    if let Some(accounts) = req.accounts.as_deref() {
        let accounts = dedup(accounts);
        check_members(&state, existing.user_id, &accounts).await?;
        account_pools::set_members(&state.pool, id, &accounts).await.map_err(|e| {
            tracing::error!("setting pool members: {e}");
            err(StatusCode::INTERNAL_SERVER_ERROR, "could not set pool members")
        })?;
    }

    let pool =
        account_pools::update(&state.pool, id, owner, name, req.strategy.as_deref(), req.failover)
            .await
            .map_err(|e| match &e {
                sqlx::Error::Database(db) if db.is_unique_violation() => {
                    err(StatusCode::CONFLICT, "a pool with that name already exists")
                }
                _ => {
                    tracing::error!("updating account pool: {e}");
                    err(StatusCode::INTERNAL_SERVER_ERROR, "could not update pool")
                }
            })?
            .ok_or_else(|| err(StatusCode::NOT_FOUND, "no such pool"))?;
    let members = account_pools::members(&state.pool, id).await.unwrap_or_default();
    Ok(Json(AccountPoolView { pool, members }))
}

/// `DELETE /account-pools/{id}`. Live sessions bound to it lose their pool
/// (`ON DELETE SET NULL`) and simply stop being movable — they keep running on
/// the account they hold.
pub async fn delete_pool(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiErr> {
    require_human(&ctx)?;
    let gone = account_pools::delete(&state.pool, id, ctx.owner_filter()).await.map_err(|e| {
        tracing::error!("deleting account pool: {e}");
        err(StatusCode::INTERNAL_SERVER_ERROR, "could not delete pool")
    })?;
    if gone { Ok(StatusCode::NO_CONTENT) } else { Err(err(StatusCode::NOT_FOUND, "no such pool")) }
}

/// `GET /sessions/{id}/rebinds` — every mid-run account move of this session.
///
/// The point of the whole feature: a session that changed accounts says so,
/// with when and why, instead of being discovered weeks later in a bill.
pub async fn list_session_rebinds(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(session_id): Path<String>,
) -> Result<Json<Vec<SessionRebind>>, ApiErr> {
    require_human(&ctx)?;
    let rebinds =
        account_pools::rebinds_for_session(&state.pool, &session_id).await.map_err(|e| {
            tracing::error!("listing session rebinds: {e}");
            err(StatusCode::INTERNAL_SERVER_ERROR, "could not list rebinds")
        })?;
    Ok(Json(rebinds))
}

/// Drop repeats while keeping first-seen order — the order is the `ordered`
/// strategy's ladder, so it must survive de-duplication.
fn dedup(ids: &[Uuid]) -> Vec<Uuid> {
    let mut seen = std::collections::HashSet::new();
    ids.iter().filter(|id| seen.insert(**id)).copied().collect()
}

/// Refuse membership the caller is not entitled to, one account at a time so
/// the error names the offender.
///
/// Two distinct refusals, because they mean different things to whoever is
/// looking: the account is not yours to use at all, or it is lent to you but
/// its owner has withheld it from pools.
async fn check_members(
    state: &AppState,
    pool_owner: Uuid,
    accounts: &[Uuid],
) -> Result<(), ApiErr> {
    for account_id in accounts {
        let row: Option<(String, bool, bool)> = sqlx::query_as(
            "SELECT a.name, a.user_id = $1 AS owned, a.pool_eligible FROM accounts a \
              WHERE a.id = $2 \
                AND (a.user_id = $1 OR EXISTS ( \
                    SELECT 1 FROM resource_shares s \
                     WHERE s.resource_type = 'account' AND s.resource_id = a.id \
                       AND s.grantee_id = $1 AND s.revoked_at IS NULL))",
        )
        .bind(pool_owner)
        .bind(account_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(|e| {
            tracing::error!("checking pool member: {e}");
            err(StatusCode::INTERNAL_SERVER_ERROR, "could not check pool members")
        })?;
        let Some((account_name, owned, pool_eligible)) = row else {
            return Err(err(StatusCode::BAD_REQUEST, "account not usable by this user"));
        };
        if !owned && !pool_eligible {
            return Err(err(
                StatusCode::BAD_REQUEST,
                &format!(
                    "account '{account_name}' is shared with you but its owner keeps it \
                     out of pools"
                ),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedup_keeps_first_seen_order() {
        let (a, b, c) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        assert_eq!(dedup(&[a, b, a, c, b]), vec![a, b, c]);
        assert_eq!(dedup(&[]), Vec::<Uuid>::new());
    }

    #[test]
    fn strategies_are_closed() {
        assert!(account_pools::valid_strategy("headroom"));
        assert!(account_pools::valid_strategy("ordered"));
        for bad in ["", "Headroom", "round-robin", "random"] {
            assert!(!account_pools::valid_strategy(bad), "{bad:?} must be rejected");
        }
    }
}
