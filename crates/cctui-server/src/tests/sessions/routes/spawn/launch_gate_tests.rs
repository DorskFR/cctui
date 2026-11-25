use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use uuid::Uuid;

use super::spawn_share_tests::{ADMIN, SPAWNER, caller, seed_machine, seed_user, share};
use crate::state::AppState;
use cctui_server_kernel::accounts_types::Family;
use cctui_server_kernel::auth::Scope;
use cctui_server_kernel::error::AppError;

const READ_ONLY: &[Scope] = &[Scope::Read];

async fn seed_session(pool: &sqlx::PgPool, owner: Uuid, machine: Uuid) -> String {
    let sid = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status, \
         adapter_id) VALUES ($1, $2, $3, $4, '/w', 'active', 'claude-code')",
    )
    .bind(&sid)
    .bind(machine.to_string())
    .bind(machine)
    .bind(owner)
    .execute(pool)
    .await
    .unwrap();
    sid
}

/// An anthropic account `user` owns; returns `(name, provider_id)`.
async fn seed_account(pool: &sqlx::PgPool, user: Uuid) -> (String, Uuid) {
    let account = Uuid::new_v4();
    let name = format!("acct-{account}");
    sqlx::query("INSERT INTO accounts (id, user_id, name) VALUES ($1, $2, $3)")
        .bind(account)
        .bind(user)
        .bind(&name)
        .execute(pool)
        .await
        .unwrap();
    let provider = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO account_providers \
           (id, user_id, account_id, provider, auth_scheme, encrypted_access_token) \
         VALUES ($1, $2, $3, 'anthropic', 'api_key', 'x')",
    )
    .bind(provider)
    .bind(user)
    .bind(account)
    .execute(pool)
    .await
    .unwrap();
    (name, provider)
}

/// A session on `owner`'s machine that `grantee` spawned on their own
/// account: minted for the grantee in the dispatch window, before the row.
async fn seed_grantee_run(
    state: &AppState,
    owner: Uuid,
    grantee: Uuid,
    machine: Uuid,
    account: &str,
) -> String {
    let sid = Uuid::new_v4().to_string();
    cctui_server_accounts::routes::gateway::mint_session_env(
        state,
        grantee,
        account,
        Family::Anthropic,
        &sid,
    )
    .await
    .expect("mint for the grantee");
    sqlx::query(
        "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status, \
         adapter_id, spawned_by) VALUES ($1, $2, $3, $4, '/w', 'active', 'claude-code', $5)",
    )
    .bind(&sid)
    .bind(machine.to_string())
    .bind(machine)
    .bind(owner)
    .bind(grantee)
    .execute(&state.pool)
    .await
    .unwrap();
    sid
}

fn code<T>(res: Result<T, AppError>) -> Result<(), StatusCode> {
    res.map(|_| ()).map_err(|e| e.into_parts().0)
}

fn daemon(
    state: &AppState,
    machine: Uuid,
) -> tokio::sync::mpsc::Receiver<cctui_proto::ws::DaemonFrameDown> {
    let (tx, rx) = tokio::sync::mpsc::channel(8);
    state.bus.register_daemon(machine, Uuid::new_v4(), tx);
    rx
}

async fn fork(
    state: &AppState,
    ctx: cctui_server_kernel::auth::AuthContext,
    sid: &str,
) -> Result<Option<String>, StatusCode> {
    cctui_server_sessions::routes::sessions::fork_session(
        State(state.clone()),
        Extension(ctx),
        Path(sid.to_owned()),
        Json(cctui_proto::api::ForkRequest::default()),
    )
    .await
    .map(|(_, Json(res))| res.session_id)
    .map_err(|e| e.into_parts().0)
}

async fn resume(
    state: &AppState,
    ctx: cctui_server_kernel::auth::AuthContext,
    sid: &str,
) -> Result<(), StatusCode> {
    code(
        cctui_server_sessions::routes::sessions::resume_session(
            State(state.clone()),
            Extension(ctx),
            Path(sid.to_owned()),
        )
        .await,
    )
}

async fn spawned_by(pool: &sqlx::PgPool, sid: &str) -> Option<Uuid> {
    sqlx::query_scalar("SELECT spawned_by FROM sessions WHERE id = $1")
        .bind(sid)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// The child must already own a credential when the daemon receives Fork,
/// including when reopening an archived parent whose tokens were revoked.
#[tokio::test]
async fn fork_binds_a_fresh_child_token_before_dispatch() {
    let Some(url) =
        cctui_server_kernel::test_support::db_url("fork_binds_a_fresh_child_token_before_dispatch")
    else {
        return;
    };
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let owner = seed_user(&pool).await;
    let machine = seed_machine(&pool, owner).await;
    let (account, provider) = seed_account(&pool, owner).await;
    let state = AppState::for_test(pool.clone());
    let mut rx = daemon(&state, machine);
    for mode in ["live", "archived", "legacy", "unbound"] {
        let parent = seed_session(&pool, owner, machine).await;
        let parent_env = if mode == "unbound" {
            std::collections::BTreeMap::new()
        } else {
            cctui_server_accounts::routes::gateway::mint_session_env(
                &state,
                owner,
                &account,
                Family::Anthropic,
                &parent,
            )
            .await
            .unwrap()
        };
        if mode == "archived" {
            sqlx::query("UPDATE session_tokens SET revoked_at = now() WHERE session_id = $1")
                .bind(&parent)
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("UPDATE sessions SET status = 'archived' WHERE id = $1")
                .bind(&parent)
                .execute(&pool)
                .await
                .unwrap();
        } else if mode == "legacy" {
            sqlx::query("DELETE FROM session_tokens WHERE session_id = $1")
                .bind(&parent)
                .execute(&pool)
                .await
                .unwrap();
        }
        let child = fork(&state, caller(owner, SPAWNER), &parent).await.unwrap().unwrap();
        let cctui_proto::ws::DaemonFrameDown::Command { command, .. } = rx.recv().await.unwrap()
        else {
            panic!("expected a fork command")
        };
        let cctui_proto::adapter::AdapterCommand::Fork { spec, session_id, .. } = *command else {
            panic!("expected a fork")
        };
        assert_eq!(session_id.as_deref(), Some(child.as_str()));
        let bindings: Vec<(Uuid, Option<Uuid>)> = sqlx::query_as(
            "SELECT account_id, user_id FROM session_tokens \
             WHERE session_id = $1 AND revoked_at IS NULL",
        )
        .bind(&child)
        .fetch_all(&pool)
        .await
        .unwrap();
        if mode == "unbound" {
            assert!(spec.env.is_empty());
            assert!(bindings.is_empty());
            continue;
        }
        assert_eq!(bindings, vec![(provider, Some(owner))]);
        assert_ne!(spec.env["ANTHROPIC_AUTH_TOKEN"], parent_env["ANTHROPIC_AUTH_TOKEN"]);
        assert_eq!(
            spec.env,
            cctui_server_accounts::routes::gateway::resume_env_for_session(&state, &child).await
        );
        let registered: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sessions WHERE id = $1)")
                .bind(&child)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(!registered, "the binding must precede registration");
    }
}

#[tokio::test]
async fn a_fork_carries_and_records_its_parents_hook_policy() {
    use cctui_proto::hook_policy::{HookEvent, HookRule, ToolClass, no_direct_harness_rule};
    let name = "a_fork_carries_and_records_its_parents_hook_policy";
    let Some(url) = cctui_server_kernel::test_support::db_url(name) else { return };
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let owner = seed_user(&pool).await;
    let machine = seed_machine(&pool, owner).await;
    let parent = seed_session(&pool, owner, machine).await;
    let deny_rm = HookRule {
        event: HookEvent::PreToolUse,
        tool: ToolClass::Shell,
        command: "deny-rm".into(),
        timeout_secs: Some(30),
    };
    let carried =
        cctui_server_accounts::hook_policy::carry("claude-code", vec![deny_rm.clone()]).unwrap();
    sqlx::query(
        "INSERT INTO machine_launches (command_id, session_id, machine_id, user_id, \
         acting_user_id, adapter_id, working_dir, hook_policy) \
         VALUES ($1, $2, $3, $4, $4, 'claude-code', '/w', $5)",
    )
    .bind(Uuid::new_v4())
    .bind(&parent)
    .bind(machine)
    .bind(owner)
    .bind(sqlx::types::Json(&carried))
    .execute(&pool)
    .await
    .unwrap();
    let state = AppState::for_test(pool.clone());
    let mut rx = daemon(&state, machine);
    let child = fork(&state, caller(owner, SPAWNER), &parent).await.unwrap().unwrap();
    let cctui_proto::ws::DaemonFrameDown::Command { command, .. } = rx.recv().await.unwrap() else {
        panic!("expected a fork command")
    };
    let cctui_proto::adapter::AdapterCommand::Fork { spec, .. } = *command else {
        panic!("expected a fork")
    };
    let rules = spec.hook_policy.expect("the fork carries a policy").rules;
    assert_eq!(rules, vec![no_direct_harness_rule(), deny_rm]);
    let recorded = cctui_server_accounts::hook_policy::for_session(&pool, &child).await.unwrap();
    assert_eq!(recorded.rules, rules, "the fork's policy is persisted under its key");
    sqlx::query("DELETE FROM users WHERE id = $1").bind(owner).execute(&pool).await.ok();
}

#[tokio::test]
async fn fork_refuses_an_inaccessible_or_deleted_account() {
    let Some(url) = cctui_server_kernel::test_support::db_url(
        "fork_refuses_an_inaccessible_or_deleted_account",
    ) else {
        return;
    };
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let owner = seed_user(&pool).await;
    let other = seed_user(&pool).await;
    let machine = seed_machine(&pool, owner).await;
    let (account, provider) = seed_account(&pool, other).await;
    sqlx::query(
        "INSERT INTO resource_shares (resource_type, resource_id, grantee_id) \
         SELECT 'account', account_id, $2 FROM account_providers WHERE id = $1",
    )
    .bind(provider)
    .bind(owner)
    .execute(&pool)
    .await
    .unwrap();
    let parent = seed_session(&pool, owner, machine).await;
    let state = AppState::for_test(pool.clone());
    cctui_server_accounts::routes::gateway::mint_session_env(
        &state,
        owner,
        &account,
        Family::Anthropic,
        &parent,
    )
    .await
    .unwrap();
    sqlx::query("UPDATE resource_shares SET revoked_at = now() WHERE grantee_id = $1")
        .bind(owner)
        .execute(&pool)
        .await
        .unwrap();
    let mut rx = daemon(&state, machine);
    assert_eq!(fork(&state, caller(owner, SPAWNER), &parent).await, Err(StatusCode::CONFLICT));
    assert!(rx.try_recv().is_err(), "never dispatch without the account");
    sqlx::query("DELETE FROM accounts WHERE name = $1 AND user_id = $2")
        .bind(&account)
        .bind(other)
        .execute(&state.pool)
        .await
        .unwrap();
    assert_eq!(fork(&state, caller(owner, SPAWNER), &parent).await, Err(StatusCode::CONFLICT));
    assert!(rx.try_recv().is_err(), "a deleted account must not become an unbound fork");
}

#[tokio::test]
async fn fork_and_resume_need_dispatch_and_a_machine_the_caller_may_spawn_on() {
    let Some(url) = cctui_server_kernel::test_support::db_url(
        "fork_and_resume_need_dispatch_and_a_machine_the_caller_may_spawn_on",
    ) else {
        return;
    };
    let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
    let owner = seed_user(&pool).await;
    let grantee = seed_user(&pool).await;
    let reader = seed_user(&pool).await;
    let stranger = seed_user(&pool).await;
    let machine = seed_machine(&pool, owner).await;
    share(&pool, machine, grantee, "spawn").await;
    share(&pool, machine, reader, "use").await;
    let state = AppState::for_test(pool.clone());
    let sid = seed_session(&pool, owner, machine).await;

    let read_only = caller(owner, READ_ONLY);
    assert_eq!(fork(&state, read_only.clone(), &sid).await, Err(StatusCode::FORBIDDEN));
    assert_eq!(resume(&state, read_only, &sid).await, Err(StatusCode::FORBIDDEN));

    let gate = |user, scopes| {
        let state = state.clone();
        async move { code(super::authorize_launch(&state, &caller(user, scopes), machine).await) }
    };
    assert_eq!(gate(owner, SPAWNER).await, Ok(()));
    assert_eq!(gate(Uuid::nil(), ADMIN).await, Ok(()));
    assert_eq!(gate(grantee, SPAWNER).await, Ok(()));
    assert_eq!(gate(reader, SPAWNER).await, Err(StatusCode::FORBIDDEN));
    assert_eq!(gate(stranger, SPAWNER).await, Err(StatusCode::FORBIDDEN));
    assert_eq!(gate(owner, READ_ONLY).await, Err(StatusCode::FORBIDDEN));

    let _daemon = daemon(&state, machine);
    let child = fork(&state, caller(owner, SPAWNER), &sid)
        .await
        .expect("the owner still forks")
        .expect("claude pre-mints the child id");
    let intent: Option<Uuid> =
        sqlx::query_scalar("SELECT user_id FROM session_spawner_intents WHERE spawn_key = $1")
            .bind(&child)
            .fetch_optional(&pool)
            .await
            .unwrap();
    assert_eq!(intent, Some(owner), "the fork is attributed to its caller");

    assert_eq!(spawned_by(&pool, &sid).await, None);
    assert_eq!(resume(&state, caller(owner, SPAWNER), &sid).await, Ok(()));
    assert_eq!(spawned_by(&pool, &sid).await, Some(owner), "a resume fills a missing spawner");
}

#[tokio::test]
async fn a_revival_never_rewrites_who_spawned_the_session() {
    let Some(url) = cctui_server_kernel::test_support::db_url(
        "a_revival_never_rewrites_who_spawned_the_session",
    ) else {
        return;
    };
    let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
    let owner = seed_user(&pool).await;
    let grantee = seed_user(&pool).await;
    let machine = seed_machine(&pool, owner).await;
    let sid = seed_session(&pool, owner, machine).await;
    super::fill_spawner(&pool, &sid, grantee).await;
    super::fill_spawner(&pool, &sid, owner).await;
    assert_eq!(spawned_by(&pool, &sid).await, Some(grantee));
}

#[tokio::test]
async fn a_grantee_run_session_stays_on_the_grantee_account() {
    let Some(url) = cctui_server_kernel::test_support::db_url(
        "a_grantee_run_session_stays_on_the_grantee_account",
    ) else {
        return;
    };
    let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
    let owner = seed_user(&pool).await;
    let grantee = seed_user(&pool).await;
    let machine = seed_machine(&pool, owner).await;
    share(&pool, machine, grantee, "spawn").await;
    let (account, provider) = seed_account(&pool, grantee).await;
    let state = AppState::for_test(pool.clone());
    let sid = seed_grantee_run(&state, owner, grantee, machine, &account).await;

    assert_eq!(
        cctui_server_accounts::routes::gateway::acting_user(&state, &sid, None).await,
        Some(grantee)
    );
    let env = cctui_server_accounts::routes::gateway::resume_env_for_session(&state, &sid).await;
    assert!(env.contains_key("ANTHROPIC_AUTH_TOKEN"), "the re-mint was refused: {env:?}");
    let live: Vec<(Uuid, Option<Uuid>)> = sqlx::query_as(
        "SELECT account_id, user_id FROM session_tokens \
         WHERE session_id = $1 AND revoked_at IS NULL",
    )
    .bind(&sid)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(live, vec![(provider, Some(grantee))]);
    assert_eq!(cctui_server_kernel::authz::session_owner(&sid, &pool).await.unwrap(), Some(owner));

    let own = seed_session(&pool, owner, machine).await;
    assert_eq!(
        cctui_server_accounts::routes::gateway::acting_user(&state, &own, None).await,
        Some(owner),
        "an unbound session still resolves to its owner"
    );
}

async fn switch(
    state: &AppState,
    ctx: cctui_server_kernel::auth::AuthContext,
    sid: &str,
    account: &str,
) -> Result<(), StatusCode> {
    code(
        cctui_server_sessions::routes::sessions::switch_account(
            State(state.clone()),
            Extension(ctx),
            Path(sid.to_owned()),
            Json(cctui_server_sessions::routes::sessions::SwitchAccountRequest {
                account: account.to_owned(),
                family: None,
            }),
        )
        .await,
    )
}

async fn binding(pool: &sqlx::PgPool, sid: &str) -> (Uuid, Option<Uuid>) {
    sqlx::query_as(
        "SELECT account_id, user_id FROM session_tokens \
         WHERE session_id = $1 AND revoked_at IS NULL",
    )
    .bind(sid)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn the_machine_owner_never_reaches_the_grantees_other_accounts() {
    let Some(url) = cctui_server_kernel::test_support::db_url(
        "the_machine_owner_never_reaches_the_grantees_other_accounts",
    ) else {
        return;
    };
    let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
    let owner = seed_user(&pool).await;
    let grantee = seed_user(&pool).await;
    let machine = seed_machine(&pool, owner).await;
    share(&pool, machine, grantee, "spawn").await;
    let (bound, _) = seed_account(&pool, grantee).await;
    let (other, other_provider) = seed_account(&pool, grantee).await;
    let (mine, my_provider) = seed_account(&pool, owner).await;
    let state = AppState::for_test(pool.clone());
    let sid = seed_grantee_run(&state, owner, grantee, machine, &bound).await;
    let as_owner = caller(owner, SPAWNER);

    assert_eq!(fork(&state, as_owner.clone(), &sid).await, Err(StatusCode::FORBIDDEN));
    let refuse = |ctx: cctui_server_kernel::auth::AuthContext| {
        let state = state.clone();
        let sid = sid.clone();
        async move { code(super::refuse_foreign_account(&state, &ctx, &sid).await) }
    };
    assert_eq!(refuse(as_owner.clone()).await, Err(StatusCode::FORBIDDEN));
    assert_eq!(refuse(caller(Uuid::nil(), ADMIN)).await, Ok(()));
    assert_eq!(refuse(caller(grantee, SPAWNER)).await, Ok(()));

    let not_found = Err(StatusCode::NOT_FOUND);
    assert_eq!(switch(&state, as_owner.clone(), &sid, &other).await, not_found);
    assert_eq!(switch(&state, caller(Uuid::nil(), ADMIN), &sid, &other).await, Ok(()));
    assert_eq!(
        binding(&pool, &sid).await,
        (other_provider, Some(grantee)),
        "an admin keeps the runner"
    );

    assert_eq!(switch(&state, as_owner.clone(), &sid, &mine).await, Ok(()));
    assert_eq!(binding(&pool, &sid).await, (my_provider, Some(owner)), "the owner took over");
    assert_eq!(switch(&state, as_owner, &sid, &other).await, not_found);
}
