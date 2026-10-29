//! Access to live resource shares used by execution policy.

use uuid::Uuid;

pub async fn granted(
    pool: &sqlx::PgPool,
    resource_type: &str,
    resource_id: Uuid,
    grantee: Uuid,
) -> Result<bool, sqlx::Error> {
    let row: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM resource_shares \
         WHERE resource_type = $1 AND resource_id = $2 AND grantee_id = $3 \
           AND revoked_at IS NULL LIMIT 1",
    )
    .bind(resource_type)
    .bind(resource_id)
    .bind(grantee)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

pub async fn granted_action(
    pool: &sqlx::PgPool,
    resource_type: &str,
    resource_id: Uuid,
    grantee: Uuid,
    action: &str,
) -> Result<bool, sqlx::Error> {
    let row: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM resource_shares \
         WHERE resource_type = $1 AND resource_id = $2 AND grantee_id = $3 \
           AND action = $4 AND revoked_at IS NULL LIMIT 1",
    )
    .bind(resource_type)
    .bind(resource_id)
    .bind(grantee)
    .bind(action)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}
