//! `/sessions/{id}/tool-guard` — lift the gateway tool guard for one session.
//! A tool policy belongs to its account's owner: only a person who is an admin,
//! or owns every account the session runs on, may toggle it. Never inherited by
//! child sessions.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use cctui_proto::api::tool_guard::{ToolGuardRequest, ToolGuardState};
use cctui_server_core::gateway::toolguard::{CompiledPolicy, ToolPolicy, invalidate_session_guard};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::auth::{AuthContext, Scope};
use crate::error::AppError;
use crate::state::AppState;

#[derive(sqlx::FromRow)]
struct SessionAccount {
    owner: Uuid,
    terms: Option<Vec<String>>,
    patterns: Option<Vec<String>>,
    protected_owners: Option<Vec<String>>,
    exempt_roots: Option<Vec<String>>,
}

impl SessionAccount {
    fn scans(&self, cwd: &str) -> bool {
        let policy = ToolPolicy {
            terms: self.terms.clone().unwrap_or_default(),
            patterns: self.patterns.clone().unwrap_or_default(),
            protected_owners: self.protected_owners.clone().unwrap_or_default(),
            exempt_roots: self.exempt_roots.clone().unwrap_or_default(),
        };
        CompiledPolicy::compile(&policy).is_some_and(|p| !p.exempts(Some(cwd)))
    }
}

type BypassRow = (String, Option<DateTime<Utc>>, Option<Uuid>, Option<String>);

async fn session_accounts(state: &AppState, id: &str) -> Result<Vec<SessionAccount>, AppError> {
    Ok(sqlx::query_as(
        "SELECT DISTINCT ON (a.id) a.user_id AS owner, p.terms, p.patterns, \
                p.protected_owners, p.exempt_roots \
         FROM session_tokens st \
         JOIN account_providers ap ON ap.id = st.account_id \
         JOIN accounts a ON a.id = ap.account_id \
         LEFT JOIN account_tool_policies p ON p.account_id = a.id \
         WHERE st.session_id = $1 AND st.revoked_at IS NULL \
         ORDER BY a.id",
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await?)
}

fn is_human(ctx: &AuthContext) -> bool {
    ctx.machine_id.is_none() && ctx.has(Scope::Read)
}

fn may_toggle(ctx: &AuthContext, accounts: &[SessionAccount]) -> bool {
    is_human(ctx) && (ctx.is_admin() || accounts.iter().all(|a| a.owner == ctx.user_id))
}

async fn load_state(
    state: &AppState,
    ctx: &AuthContext,
    id: &str,
) -> Result<ToolGuardState, AppError> {
    let row: Option<BypassRow> = sqlx::query_as(
        "SELECT s.working_dir, s.tool_guard_bypass_at, s.tool_guard_bypass_by, u.name \
         FROM sessions s LEFT JOIN users u ON u.id = s.tool_guard_bypass_by \
         WHERE s.id = $1",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((cwd, at, by_id, by)) = row else {
        return Err(AppError::new(StatusCode::NOT_FOUND, "session not found"));
    };
    let accounts = session_accounts(state, id).await?;
    let bypass = by_id.is_some();
    Ok(ToolGuardState {
        bypass,
        by: if bypass { by.or_else(|| by_id.map(|u| u.to_string())) } else { None },
        at: if bypass { at } else { None },
        policy_active: accounts.iter().any(|a| a.scans(&cwd)),
        can_toggle: may_toggle(ctx, &accounts),
    })
}

pub async fn get_tool_guard(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Json<ToolGuardState>, AppError> {
    Ok(Json(load_state(&state, &ctx, &id).await?))
}

pub async fn put_tool_guard(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
    Json(req): Json<ToolGuardRequest>,
) -> Result<Json<ToolGuardState>, AppError> {
    if !is_human(&ctx) {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "only a person can toggle the tool guard",
        ));
    }
    let accounts = session_accounts(&state, &id).await?;
    if !may_toggle(&ctx, &accounts) {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "the account's tool policy belongs to its owner",
        ));
    }
    let updated = sqlx::query(
        "UPDATE sessions SET \
             tool_guard_bypass_by = CASE WHEN $2 THEN $3::uuid END, \
             tool_guard_bypass_at = CASE WHEN $2 THEN now() END \
         WHERE id = $1",
    )
    .bind(&id)
    .bind(req.bypass)
    .bind(ctx.user_id)
    .execute(&state.pool)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(AppError::new(StatusCode::NOT_FOUND, "session not found"));
    }
    invalidate_session_guard(&id);
    tracing::info!(
        session = %id,
        user = %ctx.user_id,
        bypass = req.bypass,
        "session tool guard toggled"
    );
    Ok(Json(load_state(&state, &ctx, &id).await?))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use axum::extract::{Path, State};
    use axum::http::{Method, StatusCode};
    use axum::{Extension, Json};
    use cctui_proto::api::tool_guard::ToolGuardRequest;
    use cctui_server_core::gateway::toolguard::{GuardContext, guard_for};
    use sqlx::PgPool;
    use uuid::Uuid;

    use super::{get_tool_guard, put_tool_guard};
    use crate::auth::{AuthContext, Scope, WorkerRoute, worker_route};
    use crate::state::AppState;

    const TERM: &str = "acme";

    fn human(user_id: Uuid) -> AuthContext {
        AuthContext {
            user_id,
            key_id: Uuid::new_v4(),
            machine_id: None,
            scopes: BTreeSet::from([Scope::Read, Scope::Dispatch]),
        }
    }

    fn admin(user_id: Uuid) -> AuthContext {
        let mut ctx = human(user_id);
        ctx.scopes.insert(Scope::Admin);
        ctx
    }

    struct Fixture {
        owner: Uuid,
        driver: Uuid,
        stranger: Uuid,
        prov: Uuid,
        owned: (String, String),
        borrowed: (String, String),
    }

    async fn seed_user(pool: &PgPool, uid: Uuid) {
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("tool-guard-{uid}"))
            .bind(format!("kh-{uid}"))
            .execute(pool)
            .await
            .unwrap();
    }

    async fn seed_session(pool: &PgPool, user: Uuid, prov: Uuid) -> (String, String) {
        let machine = Uuid::new_v4();
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, $3, $4)")
            .bind(machine)
            .bind(user)
            .bind(format!("tool-guard-{machine}"))
            .bind(format!("mk-{machine}"))
            .execute(pool)
            .await
            .unwrap();
        let id = format!("tool-guard-{}", Uuid::new_v4());
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, machine_uuid, working_dir, user_id, status) \
             VALUES ($1, $2, $3, '/work/repo', $4, 'active')",
        )
        .bind(&id)
        .bind(machine.to_string())
        .bind(machine)
        .bind(user)
        .execute(pool)
        .await
        .unwrap();
        let token = format!("tok-{id}");
        sqlx::query(
            "INSERT INTO session_tokens (token_hash, session_id, account_id) VALUES ($1, $2, $3)",
        )
        .bind(crate::auth::sha256_hex(&token))
        .bind(&id)
        .bind(prov)
        .execute(pool)
        .await
        .unwrap();
        (id, token)
    }

    async fn seed(pool: &PgPool) -> Fixture {
        let (owner, driver, stranger) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        for uid in [owner, driver, stranger] {
            seed_user(pool, uid).await;
        }
        let acct = Uuid::new_v4();
        let prov = Uuid::new_v4();
        sqlx::query("INSERT INTO accounts (id, user_id, name) VALUES ($1, $2, $3)")
            .bind(acct)
            .bind(owner)
            .bind(format!("tool-guard-acct-{owner}"))
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO account_providers \
                 (id, user_id, provider, encrypted_refresh_token, account_id) \
             VALUES ($1, $2, 'anthropic', 'x', $3)",
        )
        .bind(prov)
        .bind(owner)
        .bind(acct)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO account_tool_policies (account_id, terms) VALUES ($1, $2)")
            .bind(acct)
            .bind(vec![TERM.to_owned()])
            .execute(pool)
            .await
            .unwrap();
        let owned = seed_session(pool, owner, prov).await;
        let borrowed = seed_session(pool, driver, prov).await;
        Fixture { owner, driver, stranger, prov, owned, borrowed }
    }

    async fn cleanup(pool: &PgPool, fx: &Fixture) {
        let users = vec![fx.owner, fx.driver, fx.stranger];
        for (id, _) in [&fx.owned, &fx.borrowed] {
            sqlx::query("DELETE FROM session_tokens WHERE session_id = $1")
                .bind(id)
                .execute(pool)
                .await
                .unwrap();
            sqlx::query("DELETE FROM sessions WHERE id = $1").bind(id).execute(pool).await.unwrap();
        }
        sqlx::query("DELETE FROM machines WHERE user_id = ANY($1)")
            .bind(&users)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM users WHERE id = ANY($1)")
            .bind(&users)
            .execute(pool)
            .await
            .unwrap();
    }

    async fn connect(test: &str) -> Option<PgPool> {
        let url = crate::routes::gateway::test_db_url(test)?;
        Some(sqlx::postgres::PgPoolOptions::new().max_connections(2).connect(&url).await.unwrap())
    }

    async fn put(
        pool: &PgPool,
        ctx: AuthContext,
        id: &str,
        bypass: bool,
    ) -> Result<super::ToolGuardState, StatusCode> {
        put_tool_guard(
            State(AppState::for_test(pool.clone())),
            Extension(ctx),
            Path(id.to_owned()),
            Json(ToolGuardRequest { bypass }),
        )
        .await
        .map(|Json(s)| s)
        .map_err(|e| e.status())
    }

    async fn get(pool: &PgPool, ctx: AuthContext, id: &str) -> super::ToolGuardState {
        get_tool_guard(State(AppState::for_test(pool.clone())), Extension(ctx), Path(id.to_owned()))
            .await
            .unwrap()
            .0
    }

    async fn guarded(guard: &GuardContext, prov: Uuid, token: &str) -> bool {
        guard_for(guard, prov, token).await.unwrap().is_some()
    }

    #[tokio::test]
    async fn tool_guard_auth_matrix() {
        let Some(pool) = connect("tool_guard_auth_matrix").await else { return };
        let fx = seed(&pool).await;
        let (owned, borrowed) = (&fx.owned.0, &fx.borrowed.0);

        let view = get(&pool, human(fx.owner), owned).await;
        assert!(view.policy_active && view.can_toggle && !view.bypass);
        assert!(put(&pool, human(fx.owner), owned, true).await.unwrap().bypass, "account owner");

        let view = get(&pool, human(fx.driver), borrowed).await;
        assert!(view.policy_active && !view.can_toggle, "a borrowed account is not theirs");
        assert_eq!(
            put(&pool, human(fx.driver), borrowed, true).await.unwrap_err(),
            StatusCode::FORBIDDEN,
            "a session owner who does not own the account"
        );
        assert!(put(&pool, admin(fx.stranger), borrowed, true).await.unwrap().bypass, "admin");

        assert!(
            crate::authz::authorize_session_write(&human(fx.stranger), owned, &pool).await.is_err(),
            "the route's session guard stops a user who cannot manage the session"
        );

        let mut machine_key = human(fx.owner);
        machine_key.machine_id = Some(Uuid::new_v4());
        assert_eq!(
            put(&pool, machine_key, owned, false).await.unwrap_err(),
            StatusCode::FORBIDDEN,
            "a machine key is not a person"
        );

        let state = AppState::for_test(pool.clone());
        assert!(
            state.auth_config.validate(&fx.owned.1).await.is_none(),
            "a session token is not an API credential"
        );
        assert_eq!(
            worker_route(&Method::PUT, &format!("/api/v1/sessions/{owned}/tool-guard")),
            WorkerRoute::Denied,
            "a session-bound worker key cannot reach the route"
        );

        cleanup(&pool, &fx).await;
    }

    #[tokio::test]
    async fn tool_guard_round_trip_reaches_the_gateway_at_once() {
        let Some(pool) = connect("tool_guard_round_trip_reaches_the_gateway_at_once").await else {
            return;
        };
        let fx = seed(&pool).await;
        let (id, token) = (&fx.owned.0, &fx.owned.1);
        let bus = AppState::for_test(pool.clone()).bus.clone();
        let guard = GuardContext { pool: pool.clone(), bus };

        assert!(guarded(&guard, fx.prov, token).await, "guarded before the toggle");

        let on = put(&pool, human(fx.owner), id, true).await.unwrap();
        assert!(on.bypass && on.at.is_some());
        assert_eq!(on.by.as_deref(), Some(format!("tool-guard-{}", fx.owner).as_str()));
        assert_eq!(get(&pool, human(fx.owner), id).await, on, "the state survives a reload");
        assert!(!guarded(&guard, fx.prov, token).await, "the next request is not guarded");
        assert!(
            guarded(&guard, fx.prov, &fx.borrowed.1).await,
            "another session on the account stays guarded"
        );

        let off = put(&pool, human(fx.owner), id, false).await.unwrap();
        assert!(!off.bypass && off.by.is_none() && off.at.is_none());
        assert!(guarded(&guard, fx.prov, token).await, "the next request is guarded again");

        cleanup(&pool, &fx).await;
    }
}
