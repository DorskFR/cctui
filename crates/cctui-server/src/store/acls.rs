use std::collections::BTreeSet;

use sqlx::PgExecutor;
use uuid::Uuid;

use crate::auth::Scope;

fn parse(rows: &[String]) -> BTreeSet<Scope> {
    rows.iter().filter_map(|s| Scope::parse(s)).collect()
}

/// A user's ceiling: the most any of its keys may be granted.
pub async fn user_ceiling(
    exec: impl PgExecutor<'_>,
    user_id: Uuid,
) -> Result<BTreeSet<Scope>, sqlx::Error> {
    let rows: Vec<String> = sqlx::query_scalar("SELECT scope FROM user_acls WHERE user_id = $1")
        .bind(user_id)
        .fetch_all(exec)
        .await?;
    Ok(parse(&rows))
}

/// Replace a user's ceiling. Run inside the caller's transaction.
pub async fn set_user_ceiling(
    conn: &mut sqlx::PgConnection,
    user_id: Uuid,
    scopes: &[Scope],
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM user_acls WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut *conn)
        .await?;
    let scopes: Vec<&str> = scopes.iter().copied().map(Scope::as_str).collect();
    sqlx::query("INSERT INTO user_acls (user_id, scope) SELECT $1, unnest($2::text[])")
        .bind(user_id)
        .bind(scopes)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// A key's own grant, before intersecting with its owner's ceiling.
pub async fn key_grant(
    exec: impl PgExecutor<'_>,
    key_id: Uuid,
) -> Result<BTreeSet<Scope>, sqlx::Error> {
    let rows: Vec<String> = sqlx::query_scalar("SELECT scope FROM key_acls WHERE key_id = $1")
        .bind(key_id)
        .fetch_all(exec)
        .await?;
    Ok(parse(&rows))
}

/// Add `scopes` to a key's grant in one round-trip. Idempotent.
pub async fn grant_key(
    exec: impl PgExecutor<'_>,
    key_id: Uuid,
    scopes: impl IntoIterator<Item = Scope>,
) -> Result<(), sqlx::Error> {
    let scopes: Vec<&str> = scopes.into_iter().map(Scope::as_str).collect();
    if scopes.is_empty() {
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO key_acls (key_id, scope) SELECT $1, unnest($2::text[]) \
         ON CONFLICT DO NOTHING",
    )
    .bind(key_id)
    .bind(scopes)
    .execute(exec)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn closed_pool_is_an_error_not_an_empty_ceiling() {
        let pool = sqlx::PgPool::connect_lazy("postgres://invalid").unwrap();
        pool.close().await;
        assert!(user_ceiling(&pool, Uuid::new_v4()).await.is_err());
        assert!(key_grant(&pool, Uuid::new_v4()).await.is_err());
        assert!(grant_key(&pool, Uuid::new_v4(), [Scope::Read]).await.is_err());
    }

    #[tokio::test]
    async fn grant_roundtrips_over_db() {
        let Some(url) = crate::routes::gateway::test_db_url("grant_roundtrips_over_db") else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
        let user_id: Uuid = sqlx::query_scalar(
            "INSERT INTO users (id, name, key_hash) \
             VALUES (gen_random_uuid(), $1, gen_random_uuid()::text) RETURNING id",
        )
        .bind(format!("acls-{}", Uuid::new_v4()))
        .fetch_one(&pool)
        .await
        .expect("insert user");
        sqlx::query("INSERT INTO user_acls (user_id, scope) VALUES ($1, 'read'), ($1, 'dispatch')")
            .bind(user_id)
            .execute(&pool)
            .await
            .expect("insert ceiling");
        let key_id: Uuid = sqlx::query_scalar(
            "INSERT INTO auth_keys (user_id, key_hash, kind) \
             VALUES ($1, gen_random_uuid()::text, 'user') RETURNING id",
        )
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .expect("insert key");

        let ceiling = user_ceiling(&pool, user_id).await.unwrap();
        assert_eq!(ceiling, [Scope::Read, Scope::Dispatch].into_iter().collect());
        grant_key(&pool, key_id, ceiling.clone()).await.unwrap();
        grant_key(&pool, key_id, [Scope::Read]).await.unwrap();
        grant_key(&pool, key_id, []).await.unwrap();
        assert_eq!(key_grant(&pool, key_id).await.unwrap(), ceiling);
    }
}
