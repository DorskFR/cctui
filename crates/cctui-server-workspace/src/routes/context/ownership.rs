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

/// Copy `from`'s ownership record onto `to`, leaving the context content
/// behind. With `user_id`, only a record naming that user is copied.
pub async fn inherit(pool: &PgPool, to: &str, from: &str, user_id: Option<Uuid>) {
    if to == from {
        return;
    }
    let done = cctui_telemetry::db::traced(
        "INSERT INTO session_context (spawn_key, scope) \
         SELECT $1, scope FROM session_context \
          WHERE spawn_key = $2 AND scope IS NOT NULL \
            AND ($3::text IS NULL OR scope->>'user_id' = $3) \
         ON CONFLICT (spawn_key) DO UPDATE SET scope = EXCLUDED.scope",
        |sql| {
            sqlx::query(sql).bind(to).bind(from).bind(user_id.map(|u| u.to_string())).execute(pool)
        },
    )
    .await;
    if let Err(e) = done {
        tracing::warn!(%to, error = %e, "could not copy the launch owner");
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

    #[tokio::test]
    async fn inherit_copies_the_owner_and_drops_the_content() {
        let Some(url) = crate::routes::gateway::test_db_url("launch_ownership_inherit") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let uid = Uuid::new_v4();
        let parent = format!("own-parent-{}", Uuid::new_v4());
        let (child, stranger) =
            (format!("own-child-{}", Uuid::new_v4()), format!("own-other-{}", Uuid::new_v4()));
        let scope = StoredScope {
            user_id: uid,
            coords: SpawnScope { working_dir: Some("/w".into()), ..SpawnScope::default() },
            dispatched: true,
        };
        sqlx::query(
            "INSERT INTO session_context (spawn_key, item_ids, scope, rules, memories) \
             VALUES ($1, ARRAY[$2]::uuid[], $3, '[{\"name\":\"r\"}]', '[{\"name\":\"m\"}]')",
        )
        .bind(&parent)
        .bind(Uuid::new_v4())
        .bind(Jsonb(&scope))
        .execute(&pool)
        .await
        .expect("seed the parent's context set");

        inherit(&pool, &child, &parent, None).await;
        assert_eq!(launch_scope(&pool, &child).await, Some(scope));
        test_support::assert_no_content(&pool, &child).await;

        inherit(&pool, &stranger, &parent, Some(Uuid::new_v4())).await;
        assert_eq!(owner_of(&pool, &stranger).await.expect("lookup"), None, "another user's set");

        sqlx::query("DELETE FROM session_context WHERE spawn_key = ANY($1)")
            .bind(vec![parent, child, stranger])
            .execute(&pool)
            .await
            .ok();
    }
}
