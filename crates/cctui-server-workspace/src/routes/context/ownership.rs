//! The launch-ownership record: the user a launch key belongs to and the
//! coordinates it was spawned at. It lives in the `scope` column of the
//! `session_context` row whose other columns hold the context content, and it
//! is written and read whether `context` is on or off.

use sqlx::PgPool;
use sqlx::types::Json as Jsonb;
use uuid::Uuid;

use super::SpawnScope;

/// A launch's coordinates as recorded at spawn, so every later launch of the
/// session re-resolves its rules against the owner's current set.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredScope {
    pub user_id: Uuid,
    #[serde(flatten)]
    pub coords: SpawnScope,
    /// No human is watching a dispatched worker, so everything it saves waits
    /// for approval.
    #[serde(default)]
    pub dispatched: bool,
}

/// Record who `spawn_key` belongs to without touching its context content.
/// Best-effort: a failure costs the key its ownership record, never its launch.
pub async fn record(pool: &PgPool, spawn_key: &str, scope: &StoredScope) {
    let done = cctui_telemetry::db::traced(
        "INSERT INTO session_context (spawn_key, scope) VALUES ($1, $2) \
         ON CONFLICT (spawn_key) DO UPDATE SET scope = EXCLUDED.scope",
        |sql| sqlx::query(sql).bind(spawn_key).bind(Jsonb(scope)).execute(pool),
    )
    .await;
    if let Err(e) = done {
        tracing::warn!(%spawn_key, error = %e, "could not record the launch owner");
    }
}

/// The user recorded as owning `spawn_key`, if any.
pub async fn owner_of(pool: &PgPool, spawn_key: &str) -> Result<Option<Uuid>, sqlx::Error> {
    let owner: Option<Option<String>> = cctui_telemetry::db::traced(
        "SELECT scope->>'user_id' FROM session_context WHERE spawn_key = $1",
        |sql| sqlx::query_scalar(sql).bind(spawn_key).fetch_optional(pool),
    )
    .await?;
    Ok(owner.flatten().and_then(|o| Uuid::parse_str(&o).ok()))
}

/// The coordinates `spawn_key` was launched with, if cctui launched it.
pub async fn launch_scope(pool: &PgPool, spawn_key: &str) -> Option<StoredScope> {
    let scope: Result<Option<Option<Jsonb<StoredScope>>>, sqlx::Error> =
        cctui_telemetry::db::traced(
            "SELECT scope FROM session_context WHERE spawn_key = $1",
            |sql| sqlx::query_scalar(sql).bind(spawn_key).fetch_optional(pool),
        )
        .await;
    match scope {
        Ok(scope) => scope.flatten().map(|Jsonb(s)| s),
        Err(e) => {
            tracing::warn!(%spawn_key, error = %e, "launch scope lookup failed");
            None
        }
    }
}

#[cfg(test)]
pub mod test_support {
    use sqlx::PgPool;

    pub async fn assert_no_content(pool: &PgPool, spawn_key: &str) {
        let (items, rules, memories): (i64, i64, i64) = sqlx::query_as(
            "SELECT coalesce(cardinality(item_ids), 0)::bigint, \
                    jsonb_array_length(rules)::bigint, jsonb_array_length(memories)::bigint \
               FROM session_context WHERE spawn_key = $1",
        )
        .bind(spawn_key)
        .fetch_one(pool)
        .await
        .expect("the ownership row exists");
        assert_eq!((items, rules, memories), (0, 0, 0), "{spawn_key} carries context content");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_ownership_record_carries_no_context_content() {
        let Some(url) = crate::routes::gateway::test_db_url("launch_ownership_record") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let uid = Uuid::new_v4();
        let key = format!("own-{}", Uuid::new_v4());
        let scope = StoredScope {
            user_id: uid,
            coords: SpawnScope { working_dir: Some("/w".into()), ..SpawnScope::default() },
            dispatched: true,
        };

        assert_eq!(owner_of(&pool, &key).await.expect("lookup"), None);
        assert_eq!(launch_scope(&pool, &key).await, None);
        record(&pool, &key, &scope).await;
        assert_eq!(owner_of(&pool, &key).await.expect("lookup"), Some(uid));
        assert_eq!(launch_scope(&pool, &key).await, Some(scope));
        test_support::assert_no_content(&pool, &key).await;

        sqlx::query("DELETE FROM session_context WHERE spawn_key = $1")
            .bind(&key)
            .execute(&pool)
            .await
            .ok();
    }
}
