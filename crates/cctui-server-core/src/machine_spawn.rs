//! Shared account preparation for machine execution.
use crate::account_resolve::AccountResolutionContext;
use crate::error::{AppError, DB_ERROR};
use crate::gateway::mint::MintContext;
use crate::gateway::usage::UsageContext;
use axum::Json;
use axum::http::StatusCode;
use cctui_proto::api::{ApiError, SpawnRequest};
use uuid::Uuid;

/// Execution identity to bind before a machine launch.
pub struct AccountTarget {
    pub uid: Uuid,
    pub adapter_id: String,
    pub token_session_id: String,
}

/// Account routing, credentials and quota cache shared by machine launches.
pub trait MachineAccountContext: AccountResolutionContext + MintContext + UsageContext {
    fn machine_pool(&self) -> &sqlx::PgPool {
        AccountResolutionContext::pool(self)
    }
}
impl<T: AccountResolutionContext + MintContext + UsageContext> MachineAccountContext for T {}

fn bad_request(error: impl Into<String>) -> (StatusCode, Json<ApiError>) {
    (StatusCode::BAD_REQUEST, Json(ApiError { error: error.into() }))
}
/// The account binding outcome and the session settings it may have rewritten.
pub struct BoundAccount {
    /// Request env plus any minted gateway env.
    pub env: std::collections::BTreeMap<String, String>,
    /// The session's model after per-account alias remapping (e.g. `opus` →
    /// `claude-opus-4-8[1m]`).
    pub model: Option<String>,
    pub effort: Option<String>,
    pub permission_mode: Option<cctui_proto::adapter::PermissionMode>,
    pub account_choice: Option<String>,
}

/// Resolve the launch account and mint its execution credentials.
///
/// If the caller picked a named account, mint a
/// session-scoped gateway token bound to it and inject the gateway base-url +
/// token into the worker env. Raw OAuth tokens never leave the server. With
/// no account named, see [`default_account_name`].
pub async fn resolve_spawn_account(
    state: &impl MachineAccountContext,
    target: &AccountTarget,
    req: &SpawnRequest,
) -> Result<BoundAccount, (StatusCode, Json<ApiError>)> {
    let AccountTarget { uid, adapter_id, .. } = target;
    let uid = *uid;
    let mut env = req.env.clone();
    let mut model = req.model.clone().filter(|m| !m.trim().is_empty());
    // Session-provided effort/permission_mode pass through as-is; an unset
    // field falls back to the adapter's/claude's own default.
    let effort = req.effort.clone().filter(|e| !e.trim().is_empty());
    let permission_mode = req.permission_mode;
    let decision = decide_account(
        req.account.as_deref(),
        req.no_account,
        req.auto_account,
        req.pool.as_deref(),
    );
    let auto_bound = matches!(
        decision,
        AccountDecision::ResolveDefault | AccountDecision::Auto | AccountDecision::Pool(_)
    );
    let picked_for_you = matches!(decision, AccountDecision::Auto | AccountDecision::Pool(_));
    // The pool the session may later be moved inside, stamped on its token
    // once the account is minted. `None` for every other decision: a session
    // that named no pool is never moved.
    let mut bound_pool: Option<Uuid> = None;
    let family_for_binding =
        crate::gateway::Family::from_adapter(adapter_id).map_err(|e| bad_request(e.to_string()))?;
    let account_choice = match decision {
        // A name is an account name first; it only elects a pool when no
        // account of the user's answers to it.
        AccountDecision::Named(a) => {
            let bound = crate::account_resolve::resolve_account_or_pool(
                state,
                uid,
                family_for_binding,
                model.as_deref(),
                &a,
            )
            .await
            .map_err(resolve_err)?;
            bound_pool = bound.pool_id;
            Some(bound.account)
        }
        AccountDecision::Unbound => None,
        AccountDecision::ResolveDefault => default_account_name(state, uid, adapter_id).await?,
        AccountDecision::Auto => {
            auto_account_name(state, uid, family_for_binding, model.as_deref()).await?
        }
        AccountDecision::Pool(name) => {
            let (account, pool_id) = crate::account_resolve::resolve_pool(
                state,
                uid,
                family_for_binding,
                model.as_deref(),
                &name,
            )
            .await
            .map_err(resolve_err)?;
            bound_pool = Some(pool_id);
            Some(account)
        }
    };
    if let Some(account_name) = account_choice.as_deref() {
        let acct_ref = if picked_for_you {
            format!("the auto-selected account {account_name:?}")
        } else if auto_bound {
            format!("your default account {account_name:?}")
        } else {
            format!("account {account_name:?}")
        };
        mint_account_env(state, target, account_name, &acct_ref, bound_pool, &mut env, &mut model)
            .await?;
    }
    Ok(BoundAccount { env, model, effort, permission_mode, account_choice })
}
/// Resolve `model` through the account's alias map and mint the gateway env
/// for the session. Resolution is by (account identity, harness family): the
/// adapter names the family, and the identity carries at most one provider
/// row per family.
async fn mint_account_env(
    state: &impl MachineAccountContext,
    target: &AccountTarget,
    account_name: &str,
    acct_ref: &str,
    bound_pool: Option<Uuid>,
    env: &mut std::collections::BTreeMap<String, String>,
    model: &mut Option<String>,
) -> Result<(), (StatusCode, Json<ApiError>)> {
    let AccountTarget { uid, adapter_id, token_session_id, .. } = target;
    let uid = *uid;
    let family =
        crate::gateway::Family::from_adapter(adapter_id).map_err(|e| bad_request(e.to_string()))?;
    // A harness with no default model for its provider (opencode, the
    // OpenAI-wire ACP agents) resolves even an ABSENT model: the account's
    // catalog is the only source of model ids it can launch on.
    if model.is_some() || cctui_proto::adapter::needs_model(adapter_id) {
        let requested = model.as_deref().unwrap_or_default();
        let resolved = crate::gateway::mint::resolve_account_model(
            state,
            uid,
            account_name,
            family,
            requested,
        )
        .await;
        *model = (!resolved.is_empty()).then_some(resolved);
    }
    match crate::gateway::mint::mint_session_env_all_families(
        state,
        uid,
        account_name,
        family,
        token_session_id,
    )
    .await
    {
        Ok(gateway_env) => {
            env.extend(gateway_env);
            if let Some(pool_id) = bound_pool {
                crate::account_resolve::stamp_pool(state, token_session_id, pool_id).await;
            }
            Ok(())
        }
        Err(crate::gateway::mint::MintSessionEnvError::NoAccount) => Err((
            StatusCode::NOT_FOUND,
            Json(ApiError {
                error: format!("{acct_ref} does not exist — connect it on the accounts page"),
            }),
        )),
        Err(crate::gateway::mint::MintSessionEnvError::NoProviderForFamily(f)) => Err((
            StatusCode::NOT_FOUND,
            Json(ApiError {
                error: format!(
                    "{acct_ref} has no {} provider (required by adapter \
                     {adapter_id:?}) — connect one on the accounts page",
                    f.label()
                ),
            }),
        )),
        Err(crate::gateway::mint::MintSessionEnvError::Db(e)) => {
            tracing::error!("mint_session_env failed: {e}");
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError { error: "could not provision account session".into() }),
            ))
        }
    }
}

/// Account to bind when a spawn names none: the only one (owned or shared) in
/// the adapter's family, `400` listing them if several, unbound if none.
///
/// An
/// unbound desktop worker would run on the machine owner's ambient login, so
/// the server never guesses between accounts.
pub async fn default_account_name(
    state: &impl MachineAccountContext,
    user_id: Uuid,
    adapter_id: &str,
) -> Result<Option<String>, (StatusCode, Json<ApiError>)> {
    let family =
        crate::gateway::Family::from_adapter(adapter_id).map_err(|e| bad_request(e.to_string()))?;
    let pool = state.machine_pool();
    let names: Vec<String> = cctui_telemetry::db::traced(
        "SELECT DISTINCT a.name \
         FROM account_providers ap JOIN accounts a ON a.id = ap.account_id \
         WHERE ap.family = $2 \
           AND (a.user_id = $1 OR EXISTS ( \
               SELECT 1 FROM resource_shares s \
                WHERE s.resource_type = 'account' AND s.resource_id = a.id \
                  AND s.grantee_id = $1 AND s.revoked_at IS NULL)) \
         ORDER BY a.name",
        |sql| sqlx::query_scalar(sql).bind(user_id).bind(family.label()).fetch_all(pool),
    )
    .await
    .map_err(|e| AppError::from(e).into_parts())?;
    resolve_default_account(&names, user_id, adapter_id)
}

/// One account an `auto_account` spawn may bind: account id, name, provider row
/// id, soft limits, model catalog, alias map.
pub type AutoAccountRow = (
    Uuid,
    String,
    Uuid,
    Option<serde_json::Value>,
    Option<serde_json::Value>,
    Option<serde_json::Value>,
);

/// Every account an election the caller did not name may bind in `family`.
///
/// An account whose `pool_eligible` is false is never among them: clearing it
/// is the owner saying "only when I ask for it by name" (a local model on a
/// workstation, a credential reserved for one job). Without this filter a
/// local `anthropic-compatible` or `fireworks` endpoint, which always looks
/// wide open, won any `auto_account` election its catalog did not exclude.
/// Naming the account (`account: "..."`) still binds it.
pub async fn auto_account_candidates(
    exec: impl sqlx::PgExecutor<'_>,
    user_id: Uuid,
    family: crate::gateway::Family,
    owned_only: bool,
) -> Result<Vec<AutoAccountRow>, sqlx::Error> {
    cctui_telemetry::db::traced(
        "SELECT a.id, a.name, ap.id, ap.soft_limits_json, ap.models, ap.model_aliases \
         FROM account_providers ap JOIN accounts a ON a.id = ap.account_id \
         WHERE ap.family = $2 \
           AND a.pool_eligible \
           AND (a.user_id = $1 OR (NOT $3 AND EXISTS ( \
               SELECT 1 FROM resource_shares s \
                WHERE s.resource_type = 'account' AND s.resource_id = a.id \
                  AND s.grantee_id = $1 AND s.revoked_at IS NULL))) \
         ORDER BY a.name",
        |sql| {
            sqlx::query_as(sql).bind(user_id).bind(family.label()).bind(owned_only).fetch_all(exec)
        },
    )
    .await
}

/// Pick the account an `auto_account` spawn binds: the one with the most
/// allocation left for the model it will run.
///
/// Reads every candidate the caller may use in this adapter's provider family,
/// resolves each through any live redirect chain (a rule moves new sessions to
/// another account at mint time, so ranking the origin would score an account
/// that will not serve), then reads each one's usage and hands the lot to
/// [`crate::account_pick::pick_account`].
///
/// Usage comes from the same slow-refresh per-provider cache the accounts page
/// and the gateway's soft-limit check use, so a warm cache costs nothing and a
/// cold one costs a single tokenless call per account, made concurrently. An
/// account whose usage cannot be read is ranked but never treated as
/// exhausted — see the fail-open note on `pick_account`.
pub async fn auto_account_name(
    state: &impl MachineAccountContext,
    user_id: Uuid,
    family: crate::gateway::Family,
    model: Option<&str>,
) -> Result<Option<String>, (StatusCode, Json<ApiError>)> {
    auto_account_name_scoped(state, user_id, family, model, false).await
}

/// `owned_only` drops accounts merely shared with the caller. An election the
/// user did not ask for must not spend another owner's allocation.
pub async fn auto_account_name_scoped(
    state: &impl MachineAccountContext,
    user_id: Uuid,
    family: crate::gateway::Family,
    model: Option<&str>,
    owned_only: bool,
) -> Result<Option<String>, (StatusCode, Json<ApiError>)> {
    let rows = auto_account_candidates(state.machine_pool(), user_id, family, owned_only)
        .await
        .map_err(|e| AppError::from(e).into_parts())?;
    if rows.is_empty() {
        // No accounts configured: unbound spawn, exactly as an unset `account`.
        return Ok(None);
    }

    // Redirect rules are best-effort: failing to read them must not fail the
    // spawn, it only means we rank the origin accounts.
    let rules = crate::store::account_redirects::live_for_launch(state.machine_pool(), user_id)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!("auto account: reading redirects failed, ranking origins: {e}");
            Vec::new()
        });
    // account id → the provider row whose usage represents it.
    let providers: std::collections::HashMap<Uuid, Uuid> =
        rows.iter().map(|r| (r.0, r.2)).collect();

    // Follow the chain to whoever will actually serve; fall back to the
    // account's own credential when the target is not one we can read. Usage
    // and the sessions already in flight are both measured on that credential.
    let effective: Vec<Uuid> = rows
        .iter()
        .map(|r| {
            crate::store::account_redirects::follow_account_chain(&rules, r.0, family.label())
                .and_then(|to| providers.get(&to).copied())
                .unwrap_or(r.2)
        })
        .collect();

    // An account whose catalog (the one of the credential that will serve)
    // does not list the model would answer `404 model_not_found` on the first
    // turn: a local `anthropic-compatible` endpoint always looks wide open, so
    // without this it wins every election for models it has never heard of.
    let catalogs: std::collections::HashMap<
        Uuid,
        (Option<&serde_json::Value>, Option<&serde_json::Value>),
    > = rows.iter().map(|r| (r.2, (r.4.as_ref(), r.5.as_ref()))).collect();
    let (rows, effective): (Vec<&AutoAccountRow>, Vec<Uuid>) = rows
        .iter()
        .zip(effective)
        .filter(|(_, provider)| {
            let (catalog, aliases) = catalogs.get(provider).copied().unwrap_or((None, None));
            crate::account_pick::serves_model(family, catalog, aliases, model)
        })
        .unzip();
    if rows.is_empty() {
        return Err(bad_request(format!(
            "no account can serve model {:?}: every candidate's model catalog excludes it",
            model.unwrap_or("(harness default)")
        )));
    }
    let usages = futures_util::future::join_all(effective.iter().map(|provider| async move {
        crate::gateway::usage::usage_for_soft_limit(state, *provider).await
    }))
    .await;
    let in_flight = crate::account_resolve::in_flight_by_provider(state, &effective).await;
    let reauth = crate::account_resolve::needing_reauth(state, &effective).await;

    let candidates: Vec<crate::account_pick::Candidate> = rows
        .iter()
        .zip(usages)
        .zip(&effective)
        .map(|((r, usage), provider)| crate::account_pick::Candidate {
            name: r.1.clone(),
            windows: usage
                .as_ref()
                .map(cctui_proto::api::soft_limit::normalize_usage_windows)
                .unwrap_or_default(),
            limits: cctui_proto::api::soft_limit::SoftLimits::from_json(r.3.as_ref()),
            usage_known: usage.is_some(),
            in_flight: in_flight.get(provider).copied().unwrap_or(0),
            needs_reauth: reauth.contains(provider),
        })
        .collect();

    match crate::account_pick::pick_account(&candidates, model, chrono::Utc::now()) {
        crate::account_pick::Pick::Chosen { name, headroom_pct, score, resets_at } => {
            let in_flight = candidates.iter().find(|c| c.name == name).map_or(0, |c| c.in_flight);
            tracing::info!(
                %user_id, account = %name, family = family.label(), headroom_pct, score,
                resets_at = resets_at.map(|t| t.to_rfc3339()), in_flight,
                "auto account: bound the account with the most allocation left"
            );
            Ok(Some(name))
        }
        crate::account_pick::Pick::Exhausted(blocked) => {
            let detail = blocked
                .iter()
                .map(|b| format!("{}: {}", b.name, b.reason))
                .collect::<Vec<_>>()
                .join("; ");
            Err(bad_request(format!("no account has allocation left for this session ({detail})")))
        }
        crate::account_pick::Pick::None => Ok(None),
    }
}

/// Map a shared resolver failure onto the spawn error surface.
fn resolve_err(e: crate::account_resolve::ResolveError) -> (StatusCode, Json<ApiError>) {
    match e {
        crate::account_resolve::ResolveError::Rejected(msg) => bad_request(msg),
        crate::account_resolve::ResolveError::Db => {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: DB_ERROR.into() }))
        }
    }
}

/// The account path a spawn resolves to before any DB default lookup.
#[derive(Debug, PartialEq, Eq)]
pub enum AccountDecision {
    /// The caller named an account explicitly — always binds it.
    Named(String),
    /// The caller asked for an explicit unbound spawn (`no_account`): skip
    /// `default_account_name`, run on the machine's own ambient login.
    Unbound,
    /// No account named, no unbound request: fall back to the single
    /// matching-family account, if any (auto-bind).
    ResolveDefault,
    /// The caller delegated the choice (`auto_account`): rank every candidate
    /// by remaining allocation and bind the roomiest.
    Auto,
    /// The caller named a pool: choose among its members only, by the pool's
    /// own strategy, and remember the pool on the session.
    Pool(String),
}

/// Pure so the "`no_account` bypasses default resolution" contract is testable
/// without a DB. A named account wins even if `no_account` is set, so
/// a stale flag can never suppress an explicit pick.
pub fn decide_account(
    account: Option<&str>,
    no_account: bool,
    auto_account: bool,
    pool: Option<&str>,
) -> AccountDecision {
    let pool = pool.map(str::trim).filter(|p| !p.is_empty());
    match account.map(str::trim).filter(|a| !a.is_empty()) {
        Some(a) => AccountDecision::Named(a.to_owned()),
        None if no_account => AccountDecision::Unbound,
        // A pool is a narrower instruction than `auto_account` ("these
        // accounts" vs "any account"), so it wins when both are set rather
        // than silently widening the set the caller asked for.
        None => match pool {
            Some(p) => AccountDecision::Pool(p.to_owned()),
            None if auto_account => AccountDecision::Auto,
            None => AccountDecision::ResolveDefault,
        },
    }
}

/// The 0/1/N decision over the family-filtered candidate names, split
/// from the DB query so it is unit-testable.
pub fn resolve_default_account(
    names: &[String],
    user_id: Uuid,
    adapter_id: &str,
) -> Result<Option<String>, (StatusCode, Json<ApiError>)> {
    match names {
        [] => Ok(None),
        [one] => {
            tracing::info!(%user_id, account = %one, %adapter_id, "spawn named no account — binding the user's only matching account");
            Ok(Some(one.clone()))
        }
        many => Err(bad_request(format!(
            "no account specified and several are available ({}) — pass `account` to pick one",
            many.join(", ")
        ))),
    }
}
