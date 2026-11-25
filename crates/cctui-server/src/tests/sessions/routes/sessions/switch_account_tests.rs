use std::collections::BTreeSet;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use uuid::Uuid;

use super::{SwitchAccountRequest, session_bindings, switch_account};
use crate::state::AppState;
use cctui_server_kernel::accounts_types::Family;
use cctui_server_kernel::auth::{AuthContext, Scope};
use cctui_server_kernel::error::AppError;

fn caller(user_id: Uuid, scopes: &[Scope]) -> AuthContext {
    AuthContext {
        user_id,
        key_id: Uuid::new_v4(),
        machine_id: None,
        scopes: scopes.iter().copied().collect::<BTreeSet<_>>(),
    }
}

async fn seed_user(pool: &sqlx::PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(format!("switch-{id}"))
        .bind(format!("switch-{id}-hash"))
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn seed_account(pool: &sqlx::PgPool, user: Uuid, provider: &str) -> (String, Uuid) {
    let account = Uuid::new_v4();
    let name = format!("acct-{account}");
    sqlx::query("INSERT INTO accounts (id, user_id, name) VALUES ($1, $2, $3)")
        .bind(account)
        .bind(user)
        .bind(&name)
        .execute(pool)
        .await
        .unwrap();
    let credential = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO account_providers \
           (id, user_id, account_id, provider, auth_scheme, encrypted_access_token) \
         VALUES ($1, $2, $3, $4, 'api_key', 'x')",
    )
    .bind(credential)
    .bind(user)
    .bind(account)
    .bind(provider)
    .execute(pool)
    .await
    .unwrap();
    (name, credential)
}

async fn seed_run(
    state: &AppState,
    runner: Uuid,
    claude: &str,
    codex: &str,
) -> (String, String, String) {
    let machine = Uuid::new_v4();
    sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, 'm', $3)")
        .bind(machine)
        .bind(runner)
        .bind(format!("switch-machine-{machine}"))
        .execute(&state.pool)
        .await
        .unwrap();
    let sid = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status, \
         adapter_id) VALUES ($1, $2, $3, $4, '/w', 'active', 'claude-code')",
    )
    .bind(&sid)
    .bind(machine.to_string())
    .bind(machine)
    .bind(runner)
    .execute(&state.pool)
    .await
    .unwrap();
    let claude_env = cctui_server_accounts::routes::gateway::mint_session_env(
        state,
        runner,
        claude,
        Family::Anthropic,
        &sid,
    )
    .await
    .expect("mint claude");
    let codex_env = cctui_server_accounts::routes::gateway::mint_session_env(
        state,
        runner,
        codex,
        Family::Openai,
        &sid,
    )
    .await
    .expect("mint codex");
    let claude_token = claude_env["ANTHROPIC_AUTH_TOKEN"].clone();
    let codex_token = codex_env["OPENAI_API_KEY"].clone();
    (sid, claude_token, codex_token)
}

async fn rows(pool: &sqlx::PgPool, sid: &str) -> Vec<(String, Uuid, Option<Uuid>)> {
    sqlx::query_as(
        "SELECT ap.family, t.account_id, t.user_id \
         FROM session_tokens t JOIN account_providers ap ON ap.id = t.account_id \
         WHERE t.session_id = $1 AND t.revoked_at IS NULL ORDER BY ap.family",
    )
    .bind(sid)
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn served(state: &AppState, token: &str) -> Option<Uuid> {
    cctui_server_accounts::gateway::credentials::resolve_account(state, token)
        .await
        .unwrap()
        .map(|a| a.id)
}

async fn switch(
    state: &AppState,
    ctx: AuthContext,
    sid: &str,
    account: Uuid,
) -> Result<StatusCode, AppError> {
    switch_account(
        State(state.clone()),
        Extension(ctx),
        Path(sid.to_owned()),
        Json(SwitchAccountRequest { account: account.to_string(), family: None }),
    )
    .await
}

#[tokio::test]
async fn an_admin_moves_a_foreign_run_onto_their_own_account_and_keeps_codex() {
    let Some(url) = cctui_server_kernel::test_support::db_url(
        "an_admin_moves_a_foreign_run_onto_their_own_account_and_keeps_codex",
    ) else {
        return;
    };
    let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
    cctui_server_kernel::crypto::install_vault_key(vec![7u8; 32]);
    let state = AppState::for_test(pool.clone());
    let service = seed_user(&pool).await;
    let admin = seed_user(&pool).await;
    let stranger = seed_user(&pool).await;
    let (claude, claude_cred) = seed_account(&pool, service, "anthropic").await;
    let (codex, codex_cred) = seed_account(&pool, service, "openai").await;
    let (_, personal_cred) = seed_account(&pool, admin, "anthropic").await;
    let (_, foreign_cred) = seed_account(&pool, stranger, "anthropic").await;
    let (sid, claude_token, codex_token) = seed_run(&state, service, &claude, &codex).await;
    let as_admin = caller(admin, &[Scope::Read, Scope::Dispatch, Scope::Admin]);

    let Json(bindings) =
        session_bindings(State(state.clone()), Extension(as_admin.clone()), Path(sid.clone()))
            .await
            .unwrap();
    let options = |family: &str| -> Vec<(Uuid, bool)> {
        bindings
            .iter()
            .find(|b| b.family == family)
            .map(|b| b.options.iter().map(|o| (o.credential_id, o.takeover)).collect())
            .unwrap_or_default()
    };
    assert_eq!(options("anthropic"), vec![(personal_cred, true)], "only what the server accepts");
    assert!(options("openai").is_empty(), "the admin has no other Codex account");

    let refused = switch(&state, as_admin.clone(), &sid, foreign_cred).await.unwrap_err();
    assert_eq!(refused.status(), StatusCode::NOT_FOUND);
    assert!(refused.message().contains("anthropic"), "{}", refused.message());
    assert!(refused.message().contains(&foreign_cred.to_string()), "{}", refused.message());

    let switched = switch(&state, as_admin, &sid, personal_cred).await.unwrap();
    assert_eq!(switched, StatusCode::NO_CONTENT);
    assert_eq!(
        rows(&pool, &sid).await,
        vec![
            ("anthropic".to_owned(), personal_cred, Some(admin)),
            ("openai".to_owned(), codex_cred, Some(service)),
        ],
        "the Claude binding moved to the admin, Codex stayed"
    );
    let claude_served = served(&state, &claude_token).await;
    assert_eq!(claude_served, Some(personal_cred), "no 401 on the new binding");
    assert_eq!(served(&state, &codex_token).await, Some(codex_cred));
    assert_ne!(personal_cred, claude_cred);
}

#[tokio::test]
async fn the_runner_switches_within_their_own_accounts_without_a_takeover() {
    let Some(url) = cctui_server_kernel::test_support::db_url(
        "the_runner_switches_within_their_own_accounts_without_a_takeover",
    ) else {
        return;
    };
    let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
    cctui_server_kernel::crypto::install_vault_key(vec![7u8; 32]);
    let state = AppState::for_test(pool.clone());
    let runner = seed_user(&pool).await;
    let (claude, _) = seed_account(&pool, runner, "anthropic").await;
    let (_, spare_cred) = seed_account(&pool, runner, "anthropic").await;
    let (codex, codex_cred) = seed_account(&pool, runner, "openai").await;
    let (sid, claude_token, _) = seed_run(&state, runner, &claude, &codex).await;
    let as_runner = caller(runner, &[Scope::Read, Scope::Dispatch]);

    let Json(bindings) =
        session_bindings(State(state.clone()), Extension(as_runner.clone()), Path(sid.clone()))
            .await
            .unwrap();
    let claude_options: Vec<(Uuid, bool)> = bindings
        .iter()
        .filter(|b| b.family == "anthropic")
        .flat_map(|b| b.options.iter().map(|o| (o.credential_id, o.takeover)))
        .collect();
    assert_eq!(claude_options, vec![(spare_cred, false)]);

    let switched = switch(&state, as_runner, &sid, spare_cred).await.unwrap();
    assert_eq!(switched, StatusCode::NO_CONTENT);
    assert_eq!(
        rows(&pool, &sid).await,
        vec![
            ("anthropic".to_owned(), spare_cred, Some(runner)),
            ("openai".to_owned(), codex_cred, Some(runner)),
        ]
    );
    assert_eq!(served(&state, &claude_token).await, Some(spare_cred));
}
