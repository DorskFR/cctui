#[allow(clippy::too_many_arguments)]
pub(super) async fn insert_token_usage(
    pool: &sqlx::PgPool,
    local_id: &str,
    message_id: &str,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_creation_tokens: u64,
) -> anyhow::Result<()> {
    // Cast u64 → i64 for sqlx; transcripts in the wild never approach
    // i64::MAX so saturating is safe.
    let i = i64::try_from(input_tokens).unwrap_or(i64::MAX);
    let o = i64::try_from(output_tokens).unwrap_or(i64::MAX);
    let cr = i64::try_from(cache_read_tokens).unwrap_or(i64::MAX);
    let cc = i64::try_from(cache_creation_tokens).unwrap_or(i64::MAX);
    // Opencode is metered by the gateway capture (model stamped); the daemon's
    // model-NULL row for the same message would double-count. Gate it to a no-op
    // for opencode sessions so exactly one path meters them.
    sqlx::query(
        "INSERT INTO session_token_usage \
            (session_id, message_id, input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens) \
         SELECT $1, $2, $3, $4, $5, $6 \
         WHERE NOT EXISTS ( \
             SELECT 1 FROM sessions WHERE id = $1 AND adapter_id LIKE 'opencode%') \
         ON CONFLICT (session_id, message_id) DO NOTHING",
    )
    .bind(local_id)
    .bind(message_id)
    .bind(i)
    .bind(o)
    .bind(cr)
    .bind(cc)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::routes::daemon::test_support::seed_session;

    /// DB-gated: an opencode session is metered by the gateway capture alone —
    /// the daemon's model-NULL row for the same message must be a no-op, or
    /// budgets double-count. One row survives, and it carries a model.
    #[tokio::test]
    async fn opencode_daemon_usage_is_suppressed_gateway_is_single_source() {
        let Some(url) = crate::routes::gateway::test_db_url("opencode_daemon_usage_is_suppressed")
        else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let (uid, session_id) = seed_session(&pool, "opencode", "fireworks").await;

        crate::routes::gateway::record_fireworks_usage(
            pool.clone(),
            session_id.clone(),
            Some("accounts/fireworks/models/kimi-k3".to_owned()),
            crate::cost::CapturedUsage {
                message_id: Some("gw-1".to_owned()),
                usage: crate::cost::TokenUsage { input: 100, cached_input: 0, output: 10 },
            },
            false,
        )
        .await;

        super::insert_token_usage(&pool, &session_id, "daemon-1", 100, 10, 0, 0)
            .await
            .expect("daemon insert must not error, only no-op");

        let (rows, nulls): (i64, i64) = sqlx::query_as(
            "SELECT count(*), count(*) FILTER (WHERE model IS NULL) \
             FROM session_token_usage WHERE session_id = $1",
        )
        .bind(&session_id)
        .fetch_one(&pool)
        .await
        .expect("count rows");
        assert_eq!(rows, 1, "opencode must be single-metered by the gateway capture");
        assert_eq!(nulls, 0, "the surviving row must carry the gateway-stamped model");

        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
    }

    /// DB-gated: claude-code/codex are NOT gateway-proxied, so their single
    /// daemon-reported row must still land — the opencode gate must not suppress
    /// them.
    #[tokio::test]
    async fn non_opencode_daemon_usage_is_recorded() {
        let Some(url) =
            crate::routes::gateway::test_db_url("non_opencode_daemon_usage_is_recorded")
        else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let (uid, session_id) = seed_session(&pool, "claude-code", "anthropic").await;

        super::insert_token_usage(&pool, &session_id, "daemon-1", 100, 10, 0, 0)
            .await
            .expect("daemon insert");

        let rows: i64 =
            sqlx::query_scalar("SELECT count(*) FROM session_token_usage WHERE session_id = $1")
                .bind(&session_id)
                .fetch_one(&pool)
                .await
                .expect("count rows");
        assert_eq!(rows, 1, "a non-opencode session keeps its single daemon-metered row");

        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
    }
}
