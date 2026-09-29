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

/// Revoke every live token of every session the user owns, directly or
/// through one of its machines.
pub async fn revoke_by_user(
    exec: impl PgExecutor<'_>,
    user_id: uuid::Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE session_tokens SET revoked_at = now() \
         WHERE revoked_at IS NULL AND session_id IN ( \
           SELECT s.id FROM sessions s LEFT JOIN machines m ON m.id = s.machine_uuid \
           WHERE COALESCE(s.user_id, m.user_id) = $1)",
    )
    .bind(user_id)
    .execute(exec)
    .await?;
    Ok(())
}

/// Move a session's token, any usage already metered and the spawn's bootstrap
/// attachments from `spawn_key` onto the id the harness registered under. The
/// token and usage must move together: the usage FK targets `sessions(id)`, so
/// a token left on an unregistered key meters nothing. Attachments recorded at
/// spawn under the key would otherwise never show on the session.
pub async fn rebind_session_id(
    exec: impl PgExecutor<'_> + Copy,
    spawn_key: &str,
    session_id: &str,
) -> Result<(), sqlx::Error> {
    for sql in [
        "UPDATE session_tokens SET session_id = $2 WHERE session_id = $1",
        "UPDATE session_token_usage SET session_id = $2 WHERE session_id = $1",
        "UPDATE session_attachments SET session_id = $2 WHERE session_id = $1",
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

#[cfg(test)]
mod tests {
    use sqlx::PgPool;
    use uuid::Uuid;

    async fn test_pool(test_name: &str) -> Option<PgPool> {
        let url = crate::routes::gateway::test_db_url(test_name)?;
        Some(
            sqlx::postgres::PgPoolOptions::new()
                .max_connections(2)
                .connect(&url)
                .await
                .expect("connect test db"),
        )
    }

    /// A session with one live gateway token bound to a fresh account.
    async fn session_with_token(pool: &PgPool) -> (String, Uuid) {
        let uid = Uuid::new_v4();
        let machine = Uuid::new_v4();
        let sid = Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("tok-{uid}"))
            .bind(format!("htok-{uid}"))
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, 'm', $3)")
            .bind(machine)
            .bind(uid)
            .bind(format!("mtok-{machine}"))
            .execute(pool)
            .await
            .unwrap();
        let account: Uuid =
            sqlx::query_scalar("INSERT INTO accounts (user_id, name) VALUES ($1, $2) RETURNING id")
                .bind(uid)
                .bind(format!("tok-account-{uid}"))
                .fetch_one(pool)
                .await
                .unwrap();
        let provider: Uuid = sqlx::query_scalar(
            "INSERT INTO account_providers (user_id, account_id, provider) \
             VALUES ($1, $2, 'anthropic') RETURNING id",
        )
        .bind(uid)
        .bind(account)
        .fetch_one(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status) \
             VALUES ($1, $2, $2, $3, '/w', 'archived')",
        )
        .bind(&sid)
        .bind(machine)
        .bind(uid)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO session_tokens (token_hash, session_id, account_id) \
             VALUES ($1, $2, $3)",
        )
        .bind(format!("hash-{}", Uuid::new_v4()))
        .bind(&sid)
        .bind(provider)
        .execute(pool)
        .await
        .unwrap();
        (sid, provider)
    }

    /// The contract the archive path relies on: the credential stops working,
    /// but the row stays and still names the account, so a resumed session
    /// re-mints instead of launching with an empty gateway env.
    #[tokio::test]
    async fn revoking_a_session_keeps_the_row_and_its_account() {
        let Some(pool) = test_pool("revoking_a_session_keeps_the_row_and_its_account").await else {
            return;
        };
        let (sid, provider) = session_with_token(&pool).await;

        super::revoke_by_session(&pool, &sid).await.unwrap();

        let (live, total, bound): (i64, i64, Option<String>) = sqlx::query_as(
            "SELECT count(*) FILTER (WHERE revoked_at IS NULL), count(*), max(account_id::text) \
             FROM session_tokens WHERE session_id = $1",
        )
        .bind(&sid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(live, 0, "no live token survives the revocation");
        assert_eq!(total, 1, "the row is kept, not deleted");
        assert_eq!(bound, Some(provider.to_string()), "the revoked row still names the account");
    }

    /// `archive_one` and the stale sweep can both reach the same session, and a
    /// re-archive must not re-stamp `revoked_at`: the first revocation is when
    /// the credential actually died.
    #[tokio::test]
    async fn revoking_twice_keeps_the_first_timestamp() {
        let Some(pool) = test_pool("revoking_twice_keeps_the_first_timestamp").await else {
            return;
        };
        let (sid, _) = session_with_token(&pool).await;

        super::revoke_by_session(&pool, &sid).await.unwrap();
        let first: chrono::DateTime<chrono::Utc> =
            sqlx::query_scalar("SELECT revoked_at FROM session_tokens WHERE session_id = $1")
                .bind(&sid)
                .fetch_one(&pool)
                .await
                .unwrap();
        super::revoke_by_session(&pool, &sid).await.unwrap();
        let second: chrono::DateTime<chrono::Utc> =
            sqlx::query_scalar("SELECT revoked_at FROM session_tokens WHERE session_id = $1")
                .bind(&sid)
                .fetch_one(&pool)
                .await
                .unwrap();

        assert_eq!(first, second);
    }
}
