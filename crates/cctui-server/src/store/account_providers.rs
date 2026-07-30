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

/// Resolve a provider's family scoped to the caller: it must belong to
/// `account_id`, satisfy the owner filter (`None` = admin, any owner), and not
/// be a managed row. Managed rows are read-only over the API.
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

/// Delete a provider under the same owner + `NOT managed` guard as
/// [`provider_owner_scoped`]. Returns the number of rows removed so the caller
/// can distinguish "not found / not permitted" from a real delete.
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
