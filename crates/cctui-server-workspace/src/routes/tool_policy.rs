//! `/accounts/{id}/tool-policy` — the gateway tool-call policy of an account
//! and the hook rules its sessions launch with. Only its owner (or an admin)
//! reads or edits it; the gateway policy is never sent to agents or daemons,
//! the hook rules ride every spawn the account launches.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use cctui_proto::api::tool_policy::AccountToolPolicy;
use cctui_proto::hook_policy::{HookRule, SessionHookPolicy, normalize_rules};
use uuid::Uuid;

use super::gateway::toolguard::{ToolPolicy, invalidate_policy_cache, normalize_policy};
use crate::auth::AuthContext;
use crate::error::AppError;
use crate::state::AppState;

type PolicyRow = (Vec<String>, Vec<String>, Vec<String>, Vec<String>, serde_json::Value);
const MAX_ACCOUNT_HOOKS: usize = cctui_proto::hook_policy::MAX_RULES - 1;

fn account_hooks(rules: Vec<HookRule>) -> Result<Vec<HookRule>, String> {
    let rules = normalize_rules(rules)?;
    if rules.len() > MAX_ACCOUNT_HOOKS {
        return Err(format!(
            "at most {MAX_ACCOUNT_HOOKS} hook rules per account: one slot is reserved for \
             cctui's built-in rule"
        ));
    }
    Ok(rules)
}

async fn owned(state: &AppState, ctx: &AuthContext, id: Uuid) -> Result<(), AppError> {
    let found: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM accounts WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)",
    )
    .bind(id)
    .bind(ctx.owner_filter())
    .fetch_optional(&state.pool)
    .await?;
    found.map(|_| ()).ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "no such account"))
}

fn from_row((terms, patterns, owners, roots, hooks): PolicyRow) -> AccountToolPolicy {
    AccountToolPolicy {
        guard: ToolPolicy { terms, patterns, protected_owners: owners, exempt_roots: roots },
        hooks: serde_json::from_value::<Vec<HookRule>>(hooks).unwrap_or_default(),
    }
}

pub async fn get_tool_policy(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Json<AccountToolPolicy>, AppError> {
    owned(&state, &ctx, id).await?;
    let row: Option<PolicyRow> = sqlx::query_as(
        "SELECT terms, patterns, protected_owners, exempt_roots, hooks \
         FROM account_tool_policies WHERE account_id = $1",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?;
    Ok(Json(row.map_or_else(AccountToolPolicy::default, from_row)))
}

pub async fn put_tool_policy(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    Json(req): Json<AccountToolPolicy>,
) -> Result<Json<AccountToolPolicy>, AppError> {
    owned(&state, &ctx, id).await?;
    let bad = |e: String| AppError::new(StatusCode::BAD_REQUEST, e);
    let policy = AccountToolPolicy {
        guard: normalize_policy(req.guard).map_err(bad)?,
        hooks: account_hooks(req.hooks).map_err(bad)?,
    };
    if policy == AccountToolPolicy::default() {
        sqlx::query("DELETE FROM account_tool_policies WHERE account_id = $1")
            .bind(id)
            .execute(&state.pool)
            .await?;
    } else {
        let guard = &policy.guard;
        sqlx::query(
            "INSERT INTO account_tool_policies \
                 (account_id, terms, patterns, protected_owners, exempt_roots, hooks, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, now()) \
             ON CONFLICT (account_id) DO UPDATE SET terms = EXCLUDED.terms, \
                 patterns = EXCLUDED.patterns, protected_owners = EXCLUDED.protected_owners, \
                 exempt_roots = EXCLUDED.exempt_roots, hooks = EXCLUDED.hooks, \
                 updated_at = now()",
        )
        .bind(id)
        .bind(&guard.terms)
        .bind(&guard.patterns)
        .bind(&guard.protected_owners)
        .bind(&guard.exempt_roots)
        .bind(sqlx::types::Json(&policy.hooks))
        .execute(&state.pool)
        .await?;
    }
    invalidate_policy_cache(id);
    Ok(Json(policy))
}

pub async fn get_session_hook_policy(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SessionHookPolicy>, AppError> {
    Ok(Json(cctui_server_core::hook_policy::for_session(&state.pool, &id).await?))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use axum::extract::{Path, State};
    use axum::http::StatusCode;
    use axum::{Extension, Json};
    use cctui_proto::api::tool_policy::AccountToolPolicy;
    use cctui_proto::hook_policy::{HookEvent, HookRule, ToolClass};
    use sqlx::PgPool;
    use uuid::Uuid;

    use super::{ToolPolicy, get_tool_policy, put_tool_policy};
    use crate::auth::{AuthContext, Scope};
    use crate::state::AppState;

    fn human(user_id: Uuid) -> AuthContext {
        AuthContext {
            user_id,
            key_id: Uuid::new_v4(),
            machine_id: None,
            scopes: BTreeSet::from([Scope::Read, Scope::Dispatch]),
        }
    }

    fn rule(event: HookEvent, tool: ToolClass, command: &str, timeout: Option<u32>) -> HookRule {
        HookRule { event, tool, command: command.into(), timeout_secs: timeout }
    }

    struct Fixture {
        owner: Uuid,
        account: Uuid,
        session: String,
    }

    async fn seed(pool: &PgPool) -> Fixture {
        let (owner, account, provider) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(owner)
            .bind(format!("hook-policy-{owner}"))
            .bind(format!("kh-{owner}"))
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO accounts (id, user_id, name) VALUES ($1, $2, $3)")
            .bind(account)
            .bind(owner)
            .bind(format!("hook-policy-acct-{owner}"))
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO account_providers \
                 (id, user_id, provider, encrypted_refresh_token, account_id) \
             VALUES ($1, $2, 'anthropic', 'x', $3)",
        )
        .bind(provider)
        .bind(owner)
        .bind(account)
        .execute(pool)
        .await
        .unwrap();
        let session = format!("hook-policy-{}", Uuid::new_v4());
        sqlx::query(
            "INSERT INTO session_tokens (token_hash, session_id, account_id) VALUES ($1, $2, $3)",
        )
        .bind(format!("th-{session}"))
        .bind(&session)
        .bind(provider)
        .execute(pool)
        .await
        .unwrap();
        Fixture { owner, account, session }
    }

    async fn cleanup(pool: &PgPool, fx: &Fixture) {
        sqlx::query("DELETE FROM session_tokens WHERE session_id = $1")
            .bind(&fx.session)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM users WHERE id = $1").bind(fx.owner).execute(pool).await.unwrap();
    }

    async fn put(
        pool: &PgPool,
        fx: &Fixture,
        body: AccountToolPolicy,
    ) -> Result<AccountToolPolicy, StatusCode> {
        put_tool_policy(
            State(AppState::for_test(pool.clone())),
            Extension(human(fx.owner)),
            Path(fx.account),
            Json(body),
        )
        .await
        .map(|Json(p)| p)
        .map_err(|e| e.status())
    }

    async fn get(pool: &PgPool, fx: &Fixture) -> AccountToolPolicy {
        get_tool_policy(
            State(AppState::for_test(pool.clone())),
            Extension(human(fx.owner)),
            Path(fx.account),
        )
        .await
        .unwrap()
        .0
    }

    #[test]
    fn account_hooks_leave_a_slot_for_the_built_in_rule() {
        let rules = |n: usize| -> Vec<HookRule> {
            (0..n)
                .map(|i| rule(HookEvent::PreToolUse, ToolClass::Shell, &format!("r{i}"), None))
                .collect()
        };
        assert_eq!(account_hooks(rules(MAX_ACCOUNT_HOOKS)).unwrap().len(), 63);
        let err = account_hooks(rules(MAX_ACCOUNT_HOOKS + 1)).unwrap_err();
        assert!(err.contains("one slot is reserved"), "{err}");
        let mut with_side_door = rules(MAX_ACCOUNT_HOOKS);
        with_side_door.push(cctui_proto::hook_policy::no_direct_harness_rule());
        assert!(cctui_proto::hook_policy::normalize_rules(with_side_door).is_ok());
    }

    #[tokio::test]
    async fn tool_policy_round_trips_hook_rules_and_spawns_pick_them_up() {
        let Some(url) = crate::routes::gateway::test_db_url("tool_policy_hooks") else { return };
        let pool =
            sqlx::postgres::PgPoolOptions::new().max_connections(2).connect(&url).await.unwrap();
        let fx = seed(&pool).await;

        assert_eq!(get(&pool, &fx).await, AccountToolPolicy::default());
        let deny = rule(HookEvent::PreToolUse, ToolClass::Shell, "/opt/guard/deny-rm", Some(5));
        let stop = rule(HookEvent::Stop, ToolClass::Any, "notify", None);
        let body = AccountToolPolicy {
            guard: ToolPolicy::default(),
            hooks: vec![
                HookRule { command: format!("  {}  ", deny.command), ..deny.clone() },
                HookRule { tool: ToolClass::FileEdit, ..stop.clone() },
            ],
        };
        let saved = put(&pool, &fx, body).await.unwrap();
        assert_eq!(saved.hooks, vec![deny.clone(), stop.clone()], "trimmed, tool dropped on Stop");
        assert_eq!(get(&pool, &fx).await, saved);
        assert_eq!(
            cctui_server_core::hook_policy::account_rules(&pool, &fx.session).await.unwrap(),
            vec![deny, stop]
        );

        let bad_timeout = AccountToolPolicy {
            hooks: vec![rule(HookEvent::Stop, ToolClass::Any, "x", Some(10_000))],
            ..AccountToolPolicy::default()
        };
        assert_eq!(put(&pool, &fx, bad_timeout).await, Err(StatusCode::BAD_REQUEST));

        put(&pool, &fx, AccountToolPolicy::default()).await.unwrap();
        assert_eq!(get(&pool, &fx).await, AccountToolPolicy::default());
        assert!(
            cctui_server_core::hook_policy::account_rules(&pool, &fx.session)
                .await
                .unwrap()
                .is_empty()
        );
        cleanup(&pool, &fx).await;
    }
}
