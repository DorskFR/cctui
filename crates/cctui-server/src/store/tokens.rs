use sqlx::PgExecutor;

/// Revoke every live token bound to a session. The `revoked_at IS NULL` guard
/// keeps the write idempotent and avoids re-stamping already-revoked rows.
pub async fn revoke_by_session(
    exec: impl PgExecutor<'_>,
    session_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE session_tokens SET revoked_at = now() \
         WHERE session_id = $1 AND revoked_at IS NULL",
    )
    .bind(session_id)
    .execute(exec)
    .await?;
    Ok(())
}

/// Move a session's token and any usage already metered under `spawn_key` onto
/// the id the harness registered under. Both tables must move together: the
/// usage FK targets `sessions(id)`, so a token left on an unregistered key
/// meters nothing.
pub async fn rebind_session_id(
    exec: impl PgExecutor<'_> + Copy,
    spawn_key: &str,
    session_id: &str,
) -> Result<(), sqlx::Error> {
    for sql in [
        "UPDATE session_tokens SET session_id = $2 WHERE session_id = $1",
        "UPDATE session_token_usage SET session_id = $2 WHERE session_id = $1",
    ] {
        sqlx::query(sql).bind(spawn_key).bind(session_id).execute(exec).await?;
    }
    Ok(())
}

pub async fn session_id_by_token_hash(
    exec: impl PgExecutor<'_>,
    token_hash: &str,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT session_id FROM session_tokens WHERE token_hash = $1 AND revoked_at IS NULL",
    )
    .bind(token_hash)
    .fetch_optional(exec)
    .await
}

pub async fn token_hashes_by_session(
    exec: impl PgExecutor<'_>,
    session_id: &str,
) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT token_hash FROM session_tokens WHERE session_id = $1 AND revoked_at IS NULL",
    )
    .bind(session_id)
    .fetch_all(exec)
    .await
}

pub async fn session_ids_by_account(
    exec: impl PgExecutor<'_>,
    account_id: uuid::Uuid,
) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT session_id FROM session_tokens WHERE account_id = $1 AND revoked_at IS NULL",
    )
    .bind(account_id)
    .fetch_all(exec)
    .await
}

/// Stamp `last_used_at`, throttled to at most one write per minute so the
/// gateway passthrough hot path never turns into a write per request.
pub async fn stamp_last_used(
    exec: impl PgExecutor<'_>,
    token_hash: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE session_tokens SET last_used_at = now() \
         WHERE token_hash = $1 \
           AND (last_used_at IS NULL OR last_used_at < now() - interval '60 seconds')",
    )
    .bind(token_hash)
    .execute(exec)
    .await?;
    Ok(())
}
