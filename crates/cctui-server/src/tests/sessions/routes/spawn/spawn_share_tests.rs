use axum::http::StatusCode;
use uuid::Uuid;

use cctui_server_kernel::auth::{AuthContext, Scope};

pub(super) fn caller(user_id: Uuid, scopes: &[Scope]) -> AuthContext {
    AuthContext {
        user_id,
        key_id: Uuid::new_v4(),
        machine_id: None,
        scopes: scopes.iter().copied().collect(),
    }
}

pub(super) async fn seed_user(pool: &sqlx::PgPool) -> Uuid {
    let uid = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
        .bind(uid)
        .bind(format!("share-spawn-{uid}"))
        .bind(format!("hss-{uid}"))
        .execute(pool)
        .await
        .unwrap();
    uid
}

pub(super) async fn seed_machine(pool: &sqlx::PgPool, owner: Uuid) -> Uuid {
    let machine = Uuid::new_v4();
    sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, 'm', $3)")
        .bind(machine)
        .bind(owner)
        .bind(format!("mkss-{machine}"))
        .execute(pool)
        .await
        .unwrap();
    machine
}

pub(super) async fn share(pool: &sqlx::PgPool, machine: Uuid, grantee: Uuid, action: &str) {
    sqlx::query(
        "INSERT INTO resource_shares (resource_type, resource_id, grantee_id, action) \
         VALUES ('machine', $1, $2, $3)",
    )
    .bind(machine)
    .bind(grantee)
    .bind(action)
    .execute(pool)
    .await
    .unwrap();
}

pub(super) const SPAWNER: &[Scope] = &[Scope::Read, Scope::Enroll, Scope::Dispatch];
pub(super) const ADMIN: &[Scope] = &[Scope::Read, Scope::Enroll, Scope::Dispatch, Scope::Admin];

#[tokio::test]
async fn a_spawn_needs_the_dispatch_scope() {
    let pool = sqlx::PgPool::connect_lazy("postgres://invalid").unwrap();
    let state = crate::state::AppState::for_test(pool);
    let req: cctui_proto::api::SpawnRequest = serde_json::from_value(serde_json::json!({
        "machine_id": Uuid::new_v4().to_string(),
        "working_dir": "/w",
        "permission_mode": "yolo",
    }))
    .unwrap();
    let read_only = caller(Uuid::new_v4(), &[Scope::Read]);
    let refused = super::dispatch_spawn(&state, &read_only, req, Vec::new(), Vec::new()).await;
    assert_eq!(refused.err().map(|(code, _)| code), Some(StatusCode::FORBIDDEN));
}

#[tokio::test]
async fn only_a_spawn_share_lets_a_non_owner_spawn() {
    let Some(url) =
        cctui_server_kernel::test_support::db_url("only_a_spawn_share_lets_a_non_owner_spawn")
    else {
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
    let state = crate::state::AppState::for_test(pool.clone());
    let id = machine.to_string();
    let resolve = |user, scopes| {
        let state = state.clone();
        let id = id.clone();
        async move {
            super::resolve_spawn_machine(&state, &caller(user, scopes), &id)
                .await
                .map_err(|(code, _)| code)
        }
    };

    assert_eq!(resolve(owner, SPAWNER).await, Ok((machine, owner)));
    assert_eq!(resolve(Uuid::nil(), ADMIN).await, Ok((machine, owner)));
    assert_eq!(
        resolve(grantee, SPAWNER).await,
        Ok((machine, owner)),
        "a spawn grantee spawns, and the machine owner keeps the session"
    );
    assert_eq!(resolve(reader, SPAWNER).await, Err(StatusCode::FORBIDDEN));
    assert_eq!(resolve(stranger, SPAWNER).await, Err(StatusCode::FORBIDDEN));
    assert_eq!(
        super::resolve_owned_machine(&state, &caller(grantee, SPAWNER), &id)
            .await
            .map_err(|(code, _)| code),
        Err(StatusCode::FORBIDDEN),
        "drafts and other owner-only paths do not honour a spawn share"
    );

    sqlx::query(
        "UPDATE resource_shares SET revoked_at = now() \
         WHERE resource_type = 'machine' AND resource_id = $1 AND grantee_id = $2",
    )
    .bind(machine)
    .bind(grantee)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(resolve(grantee, SPAWNER).await, Err(StatusCode::FORBIDDEN));
}

#[tokio::test]
async fn the_spawner_is_recorded_and_the_owner_keeps_the_session() {
    let Some(url) = cctui_server_kernel::test_support::db_url(
        "the_spawner_is_recorded_and_the_owner_keeps_the_session",
    ) else {
        return;
    };
    let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
    let owner = seed_user(&pool).await;
    let grantee = seed_user(&pool).await;
    let machine = seed_machine(&pool, owner).await;
    share(&pool, machine, grantee, "spawn").await;
    let spawn_key = Uuid::new_v4().to_string();
    let sid = Uuid::new_v4().to_string();

    super::remember_spawner(&pool, &spawn_key, grantee, None).await;
    sqlx::query(
        "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status) \
         VALUES ($1, $2, $3, $4, '/w', 'new')",
    )
    .bind(&sid)
    .bind(machine.to_string())
    .bind(machine)
    .bind(owner)
    .execute(&pool)
    .await
    .unwrap();
    super::claim_spawner(&pool, &sid, Some(&spawn_key)).await;
    super::claim_spawner(&pool, &sid, None).await;

    let spawned_by: Option<Uuid> =
        sqlx::query_scalar("SELECT spawned_by FROM sessions WHERE id = $1")
            .bind(&sid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(spawned_by, Some(grantee), "a second claim must not clear it");
    let left: i64 =
        sqlx::query_scalar("SELECT count(*) FROM session_spawner_intents WHERE spawn_key = $1")
            .bind(&spawn_key)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(left, 0, "the intent is consumed");

    assert_eq!(cctui_server_kernel::authz::session_owner(&sid, &pool).await.unwrap(), Some(owner));
    assert_eq!(
        cctui_server_kernel::authz::authorize_session_read(&caller(owner, SPAWNER), &sid, &pool)
            .await,
        Ok(())
    );
    assert_eq!(
        cctui_server_kernel::authz::authorize_session_read(&caller(grantee, SPAWNER), &sid, &pool)
            .await,
        Err(StatusCode::FORBIDDEN),
        "spawning through a share does not make the grantee an owner"
    );
}

#[tokio::test]
async fn a_spawn_name_is_pinned_when_the_session_registers() {
    let Some(url) = cctui_server_kernel::test_support::db_url(
        "a_spawn_name_is_pinned_when_the_session_registers",
    ) else {
        return;
    };
    let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
    let owner = seed_user(&pool).await;
    let machine = seed_machine(&pool, owner).await;
    let register = |name: Option<&'static str>| {
        let pool = pool.clone();
        async move {
            let sid = Uuid::new_v4().to_string();
            super::remember_spawner(&pool, &sid, owner, name).await;
            sqlx::query(
                "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, \
                 status) VALUES ($1, $2, $3, $4, '/w', 'new')",
            )
            .bind(&sid)
            .bind(machine.to_string())
            .bind(machine)
            .bind(owner)
            .execute(&pool)
            .await
            .unwrap();
            super::claim_spawner(&pool, &sid, None).await;
            let row: (Option<String>, Option<String>) = sqlx::query_as(
                "SELECT session_name, metadata->>'name_source' FROM sessions WHERE id = $1",
            )
            .bind(&sid)
            .fetch_one(&pool)
            .await
            .unwrap();
            row
        }
    };

    assert_eq!(
        register(Some("  SUPER ORCHESTRATOR ")).await,
        (Some("SUPER ORCHESTRATOR".to_owned()), Some("user".to_owned()))
    );
    assert_eq!(register(None).await, (None, None), "no name, nothing pinned");
    assert_eq!(register(Some("   ")).await, (None, None), "a blank name is no name");
}

async fn intents_under(pool: &sqlx::PgPool, table: &str, key: &str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT count(*) FROM {table} WHERE spawn_key = $1"
    )))
    .bind(key)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// `validate_spawn` returns only once every intent is written, and
/// `execute_spawn` is what delivers the command, so a daemon answering
/// `SessionStarted` at once still finds all of them.
#[tokio::test]
async fn every_spawn_intent_is_written_before_the_spawn_is_delivered() {
    let Some(url) = cctui_server_kernel::test_support::db_url(
        "every_spawn_intent_is_written_before_the_spawn_is_delivered",
    ) else {
        return;
    };
    let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
    let owner = seed_user(&pool).await;
    let machine = seed_machine(&pool, owner).await;
    let label: Uuid = sqlx::query_scalar(
        "INSERT INTO labels (name, color, user_id) VALUES ($1, '#abcdef', $2) RETURNING id",
    )
    .bind(format!("intent-{owner}"))
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    let parent = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status) \
         VALUES ($1, $2, $3, $4, '/w', 'active')",
    )
    .bind(&parent)
    .bind(machine.to_string())
    .bind(machine)
    .bind(owner)
    .execute(&pool)
    .await
    .unwrap();
    let state = crate::state::AppState::for_test(pool.clone());
    let req: cctui_proto::api::SpawnRequest = serde_json::from_value(serde_json::json!({
        "machine_id": machine.to_string(),
        "working_dir": "/w",
        "name": "intents",
        "auto_archive": true,
        "label_ids": [label.to_string()],
        "relation": cctui_server_sessions::followup::RELATION,
        "parent_session_id": parent,
    }))
    .unwrap();

    let target = super::validate_spawn(&state, &caller(owner, SPAWNER), &req, Uuid::new_v4())
        .await
        .map_err(|(code, _)| code)
        .expect("a valid spawn");
    let key = target.token_session_id;
    for table in [
        "session_auto_archive",
        "session_spawner_intents",
        "session_label_intents",
        "session_followup_intents",
        "session_context",
    ] {
        assert_eq!(intents_under(&pool, table, &key).await, 1, "{table}");
    }

    for table in [
        "session_auto_archive",
        "session_spawner_intents",
        "session_label_intents",
        "session_followup_intents",
        "session_context",
    ] {
        sqlx::query(sqlx::AssertSqlSafe(format!("DELETE FROM {table} WHERE spawn_key = $1")))
            .bind(&key)
            .execute(&pool)
            .await
            .ok();
    }
    sqlx::query("DELETE FROM sessions WHERE id = $1").bind(&parent).execute(&pool).await.ok();
    sqlx::query("DELETE FROM labels WHERE id = $1").bind(label).execute(&pool).await.ok();
}

/// Recording a spawn no longer purges; the reaper sweep drops intents
/// nobody claimed within a day and keeps the rest.
#[tokio::test]
async fn only_the_sweep_purges_day_old_spawner_intents() {
    let Some(url) =
        cctui_server_kernel::test_support::db_url("only_the_sweep_purges_day_old_spawner_intents")
    else {
        return;
    };
    let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
    let owner = seed_user(&pool).await;
    let stale = Uuid::new_v4().to_string();
    let fresh = Uuid::new_v4().to_string();
    super::remember_spawner(&pool, &stale, owner, None).await;
    sqlx::query(
        "UPDATE session_spawner_intents SET created_at = now() - interval '2 days' \
         WHERE spawn_key = $1",
    )
    .bind(&stale)
    .execute(&pool)
    .await
    .unwrap();
    super::remember_spawner(&pool, &fresh, owner, None).await;
    assert_eq!(
        intents_under(&pool, "session_spawner_intents", &stale).await,
        1,
        "recording a spawn leaves the purge to the sweep"
    );

    cctui_server_dispatch::machine_policy::purge_spawner_intents(&pool).await;
    assert_eq!(intents_under(&pool, "session_spawner_intents", &stale).await, 0);
    assert_eq!(intents_under(&pool, "session_spawner_intents", &fresh).await, 1);

    sqlx::query("DELETE FROM users WHERE id = $1").bind(owner).execute(&pool).await.ok();
}
