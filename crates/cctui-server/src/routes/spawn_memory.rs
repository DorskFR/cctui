//! `/api/v1/spawn-memory` — the last spawn configuration per target.
//!
//! The map is stored as `spawnMemory` inside the `user_settings` JSON blob;
//! these routes project that one key, so a client need not pull or rewrite the
//! whole blob to recall one entry. [`clamp`] guards both write paths.

use std::collections::BTreeMap;

use axum::extract::State;
use axum::{Extension, Json};
use cctui_proto::drafts::{SPAWN_MEMORY_CAP, SpawnMemoryEntry, SpawnMemoryPayload};
use serde_json::Value;

use crate::auth::AuthContext;
use crate::error::AppError;
use crate::state::AppState;

/// Parse the `spawnMemory` key of a settings blob, dropping entries that no
/// longer deserialize rather than failing the whole read.
#[must_use]
pub fn of(data: &Value) -> BTreeMap<String, SpawnMemoryEntry> {
    let mut entries = BTreeMap::new();
    let Some(obj) = data.get("spawnMemory").and_then(Value::as_object) else { return entries };
    for (key, raw) in obj {
        if let Ok(entry) = serde_json::from_value::<SpawnMemoryEntry>(raw.clone()) {
            entries.insert(key.clone(), entry);
        }
    }
    entries
}

/// Cap `data.spawnMemory` in place, removing the key when it holds nothing, so
/// a stored blob never grows past [`SPAWN_MEMORY_CAP`] entries.
pub fn clamp(data: &mut Value) {
    let Some(obj) = data.as_object_mut() else { return };
    if !obj.contains_key("spawnMemory") {
        return;
    }
    let mut entries = of(&Value::Object(obj.clone()));
    cctui_proto::drafts::evict_spawn_memory(&mut entries, SPAWN_MEMORY_CAP);
    if entries.is_empty() {
        obj.remove("spawnMemory");
        return;
    }
    if let Ok(v) = serde_json::to_value(&entries) {
        obj.insert("spawnMemory".to_owned(), v);
    }
}

pub async fn get_spawn_memory(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<SpawnMemoryPayload>, AppError> {
    let data: Option<Value> =
        sqlx::query_scalar("SELECT data FROM user_settings WHERE user_id = $1")
            .bind(ctx.user_id)
            .fetch_optional(&state.pool)
            .await?;
    let entries = data.as_ref().map(of).unwrap_or_default();
    Ok(Json(SpawnMemoryPayload { entries }))
}

/// Replace the whole map. The caller sends what it wants kept; the server caps
/// it and leaves every other settings key untouched.
pub async fn put_spawn_memory(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(body): Json<SpawnMemoryPayload>,
) -> Result<Json<SpawnMemoryPayload>, AppError> {
    let mut entries = body.entries;
    cctui_proto::drafts::evict_spawn_memory(&mut entries, SPAWN_MEMORY_CAP);
    let stored = serde_json::to_value(&entries)?;
    sqlx::query(
        "INSERT INTO user_settings (user_id, version, data) \
         VALUES ($1, 1, jsonb_build_object('spawnMemory', $2::jsonb)) \
         ON CONFLICT (user_id) DO UPDATE \
         SET data = jsonb_set(user_settings.data, '{spawnMemory}', $2::jsonb, true), \
             updated_at = now()",
    )
    .bind(ctx.user_id)
    .bind(&stored)
    .execute(&state.pool)
    .await?;
    Ok(Json(SpawnMemoryPayload { entries }))
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn of_reads_known_entries_and_skips_unparseable_ones() {
        let data = json!({ "spawnMemory": { "a": entry(1), "b": "not an entry" } });
        let entries = of(&data);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries["a"].model_claude, "opus");
        assert!(of(&json!({})).is_empty());
    }

    #[test]
    fn clamp_caps_the_map_and_leaves_sibling_settings_alone() {
        let mut memory = serde_json::Map::new();
        for i in 0..=i64::try_from(SPAWN_MEMORY_CAP).unwrap() {
            memory.insert(format!("m-{i}"), entry(i));
        }
        let mut data = json!({ "spawnMemory": memory, "theme": "dark" });
        clamp(&mut data);
        let kept = data["spawnMemory"].as_object().unwrap();
        assert_eq!(kept.len(), SPAWN_MEMORY_CAP);
        assert!(!kept.contains_key("m-0"), "the oldest entry is evicted");
        assert_eq!(data["theme"], "dark");
    }

    #[test]
    fn clamp_drops_an_empty_block_and_ignores_an_absent_one() {
        let mut empty = json!({ "spawnMemory": {} });
        clamp(&mut empty);
        assert!(empty.get("spawnMemory").is_none());

        let mut absent = json!({ "theme": "dark" });
        clamp(&mut absent);
        assert!(absent.get("spawnMemory").is_none());
    }

    fn ctx(user_id: Uuid) -> AuthContext {
        AuthContext { user_id, key_id: Uuid::new_v4(), machine_id: None, scopes: BTreeSet::new() }
    }

    async fn test_pool(test_name: &str) -> Option<sqlx::PgPool> {
        let url = crate::routes::gateway::test_db_url(test_name)?;
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
        let Some(pool) = test_pool("put_preserves_the_other_settings_keys_and_reads_back").await
        else {
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
}
