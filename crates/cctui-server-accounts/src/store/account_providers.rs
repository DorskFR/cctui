use sqlx::PgExecutor;
use uuid::Uuid;

pub async fn provider_by_id(
    exec: impl PgExecutor<'_>,
    id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT provider FROM account_providers WHERE id = $1")
        .bind(id)
        .fetch_optional(exec)
        .await
}

/// Resolve a provider's family scoped to the caller: it must belong to `account_id`, satisfy
/// the owner filter (`None` = admin, any owner), and not be a managed row.
///
/// Managed rows are read-only over the API.
pub async fn provider_owner_scoped(
    exec: impl PgExecutor<'_>,
    provider_id: Uuid,
    account_id: Uuid,
    owner: Option<Uuid>,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT provider FROM account_providers \
         WHERE id = $1 AND account_id = $2 \
           AND ($3::uuid IS NULL OR user_id = $3) AND NOT managed",
    )
    .bind(provider_id)
    .bind(account_id)
    .bind(owner)
    .fetch_optional(exec)
    .await
}

/// Delete a provider under the same owner + `NOT managed` guard as [`provider_owner_scoped`].
///
/// Returns the number of rows removed so the caller can distinguish "not found / not
/// permitted" from a real delete.
pub async fn delete_owner_scoped(
    exec: impl PgExecutor<'_>,
    provider_id: Uuid,
    account_id: Uuid,
    owner: Option<Uuid>,
) -> Result<u64, sqlx::Error> {
    let res = sqlx::query(
        "DELETE FROM account_providers \
         WHERE id = $1 AND account_id = $2 \
           AND ($3::uuid IS NULL OR user_id = $3) AND NOT managed",
    )
    .bind(provider_id)
    .bind(account_id)
    .bind(owner)
    .execute(exec)
    .await?;
    Ok(res.rows_affected())
}

/// A provider's name when the caller may read it: `owner = None` (admin), the
/// owner, or a live share grantee of its account. Read access only.
pub async fn provider_readable(
    exec: impl PgExecutor<'_>,
    provider_id: Uuid,
    owner: Option<Uuid>,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT p.provider FROM account_providers p \
         WHERE p.id = $1 \
           AND ($2::uuid IS NULL OR p.user_id = $2 \
                OR EXISTS (SELECT 1 FROM resource_shares s \
                           WHERE s.resource_type = 'account' AND s.resource_id = p.account_id \
                             AND s.grantee_id = $2 AND s.revoked_at IS NULL))",
    )
    .bind(provider_id)
    .bind(owner)
    .fetch_optional(exec)
    .await
}

#[cfg(test)]
mod tests {
    use super::provider_readable;
    use cctui_server_kernel::schema::core::bootstrap;
    use cctui_server_kernel::test_support::with_isolated_database;
    use sqlx::PgPool;
    use uuid::Uuid;

    async fn user(pool: &PgPool, name: &str) -> Uuid {
        sqlx::query_scalar("INSERT INTO users (name, key_hash) VALUES ($1, $2) RETURNING id")
            .bind(name)
            .bind(format!("kh-{name}"))
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn shared_provider(pool: &PgPool, owner: Uuid, grantee: Uuid) -> (Uuid, Uuid) {
        let account: Uuid = sqlx::query_scalar(
            "INSERT INTO accounts (user_id, name) VALUES ($1, 'shared') RETURNING id",
        )
        .bind(owner)
        .fetch_one(pool)
        .await
        .unwrap();
        let provider: Uuid = sqlx::query_scalar(
            "INSERT INTO account_providers (user_id, account_id, provider) \
             VALUES ($1, $2, 'anthropic') RETURNING id",
        )
        .bind(owner)
        .bind(account)
        .fetch_one(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO resource_shares (resource_type, resource_id, grantee_id) \
             VALUES ('account', $1, $2)",
        )
        .bind(account)
        .bind(grantee)
        .execute(pool)
        .await
        .unwrap();
        (account, provider)
    }

    #[tokio::test]
    async fn provider_readable_by_owner_admin_and_live_grantee_only() {
        with_isolated_database("provider_readable_shares", |pool| async move {
            bootstrap(&pool).await.unwrap();
            let owner = user(&pool, "owner").await;
            let grantee = user(&pool, "grantee").await;
            let stranger = user(&pool, "stranger").await;
            let (account, provider) = shared_provider(&pool, owner, grantee).await;
            let read = |who: Option<Uuid>| provider_readable(&pool, provider, who);

            assert_eq!(read(Some(owner)).await.unwrap().as_deref(), Some("anthropic"));
            assert_eq!(read(None).await.unwrap().as_deref(), Some("anthropic"));
            assert_eq!(read(Some(grantee)).await.unwrap().as_deref(), Some("anthropic"));
            assert_eq!(read(Some(stranger)).await.unwrap(), None);
            assert_eq!(provider_readable(&pool, Uuid::new_v4(), None).await.unwrap(), None);

            sqlx::query(
                "UPDATE resource_shares SET revoked_at = now() \
                 WHERE resource_type = 'account' AND resource_id = $1",
            )
            .bind(account)
            .execute(&pool)
            .await
            .unwrap();
            assert_eq!(read(Some(grantee)).await.unwrap(), None);
            assert_eq!(read(Some(owner)).await.unwrap().as_deref(), Some("anthropic"));
        })
        .await;
    }
}
