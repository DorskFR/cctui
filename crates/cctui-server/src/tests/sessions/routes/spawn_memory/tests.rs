use std::collections::BTreeSet;

use serde_json::json;
use uuid::Uuid;

use super::*;

fn entry(at: i64) -> Value {
    json!({
        "adapter_id": "claude-code",
        "model_claude": "opus",
        "model_codex": "",
        "model_account": "",
        "effort_claude": "high",
        "effort_codex": "",
        "account": "acct",
        "account_provider": "anthropic",
        "permission_mode": "ask",
        "name": "run",
        "at": at
    })
}

fn ctx(user_id: Uuid) -> AuthContext {
    AuthContext { user_id, key_id: Uuid::new_v4(), machine_id: None, scopes: BTreeSet::new() }
}

async fn test_pool(test_name: &str) -> Option<sqlx::PgPool> {
    let url = cctui_server_kernel::test_support::db_url(test_name)?;
    Some(
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db"),
    )
}

async fn insert_user(pool: &sqlx::PgPool, tag: &str) -> Uuid {
    let uid = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
        .bind(uid)
        .bind(format!("{tag}-{uid}"))
        .bind(format!("h-{tag}-{uid}"))
        .execute(pool)
        .await
        .unwrap();
    uid
}

/// A spawn-memory PUT must not be a settings wipe: the projection writes one
/// key of the blob and leaves the rest of the user's settings standing.
#[tokio::test]
async fn put_preserves_the_other_settings_keys_and_reads_back() {
    let Some(pool) = test_pool("put_preserves_the_other_settings_keys_and_reads_back").await else {
        return;
    };
    let uid = insert_user(&pool, "spawnmem").await;
    sqlx::query("INSERT INTO user_settings (user_id, version, data) VALUES ($1, 1, $2)")
        .bind(uid)
        .bind(json!({ "theme": "dark" }))
        .execute(&pool)
        .await
        .unwrap();
    let state = AppState::for_test(pool.clone());

    let mut entries = BTreeMap::new();
    entries.insert(
        "m\u{1f}mach\u{1f}/w".to_owned(),
        serde_json::from_value::<SpawnMemoryEntry>(entry(7)).unwrap(),
    );
    let put = put_spawn_memory(
        State(state.clone()),
        Extension(ctx(uid)),
        Json(SpawnMemoryPayload { entries }),
    )
    .await
    .unwrap();
    assert_eq!(put.0.entries.len(), 1);

    let got = get_spawn_memory(State(state), Extension(ctx(uid))).await.unwrap();
    assert_eq!(got.0.entries["m\u{1f}mach\u{1f}/w"].at, 7);

    let data: Value = sqlx::query_scalar("SELECT data FROM user_settings WHERE user_id = $1")
        .bind(uid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(data["theme"], "dark", "the PUT must not clobber sibling settings");
}

#[tokio::test]
async fn spawn_memory_is_scoped_to_its_owner() {
    let Some(pool) = test_pool("spawn_memory_is_scoped_to_its_owner").await else {
        return;
    };
    let owner = insert_user(&pool, "spawnmem-owner").await;
    let other = insert_user(&pool, "spawnmem-other").await;
    let state = AppState::for_test(pool);

    let mut entries = BTreeMap::new();
    entries.insert(
        "m\u{1f}mach\u{1f}/secret".to_owned(),
        serde_json::from_value::<SpawnMemoryEntry>(entry(1)).unwrap(),
    );
    let stored = put_spawn_memory(
        State(state.clone()),
        Extension(ctx(owner)),
        Json(SpawnMemoryPayload { entries }),
    )
    .await
    .unwrap();
    assert_eq!(stored.0.entries.len(), 1);

    let seen = get_spawn_memory(State(state), Extension(ctx(other))).await.unwrap();
    assert!(seen.0.entries.is_empty());
}
