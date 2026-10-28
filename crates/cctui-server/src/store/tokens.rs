//! Gateway tokens plus full-server attachment rebinding.

pub use cctui_server_core::store::tokens::*;

/// Rebind execution records, then preserve the full server's uploaded attachments.
#[cfg(test)]
pub async fn rebind_session_id(
    exec: impl sqlx::PgExecutor<'_> + Copy,
    spawn_key: &str,
    session_id: &str,
) -> Result<(), sqlx::Error> {
    cctui_server_core::store::tokens::rebind_session_id(exec, spawn_key, session_id).await?;
    rebind_attachments(exec, spawn_key, session_id).await
}

/// Workspace records follow a successful execution-record rebind.
pub async fn rebind_attachments(
    exec: impl sqlx::PgExecutor<'_>,
    spawn_key: &str,
    session_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE session_attachments SET session_id = $2 WHERE session_id = $1 \
           AND NOT EXISTS (SELECT 1 FROM sessions WHERE id = $1)",
    )
    .bind(spawn_key)
    .bind(session_id)
    .execute(exec)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn full_server_rebinds_attachments_and_preserves_live_session_records() {
        let Some(url) = crate::routes::gateway::test_db_url("full_token_rebind") else { return };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .after_connect(|conn, _| {
                Box::pin(async move {
                    sqlx::query("SET search_path = pg_temp").execute(conn).await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        sqlx::raw_sql(
            "CREATE TEMP TABLE sessions (id text); \
             CREATE TEMP TABLE session_tokens (session_id text); \
             CREATE TEMP TABLE session_token_usage (session_id text); \
             CREATE TEMP TABLE session_attachments (session_id text); \
             INSERT INTO sessions VALUES ('live'); \
             INSERT INTO session_tokens VALUES ('spawn'), ('live'); \
             INSERT INTO session_token_usage VALUES ('spawn'), ('live'); \
             INSERT INTO session_attachments VALUES ('spawn'), ('live');",
        )
        .execute(&pool)
        .await
        .unwrap();
        super::rebind_session_id(&pool, "spawn", "registered").await.unwrap();
        super::rebind_session_id(&pool, "live", "stolen").await.unwrap();
        for sql in [
            "SELECT session_id FROM session_tokens ORDER BY session_id",
            "SELECT session_id FROM session_token_usage ORDER BY session_id",
            "SELECT session_id FROM session_attachments ORDER BY session_id",
        ] {
            let ids: Vec<String> = sqlx::query_scalar(sql).fetch_all(&pool).await.unwrap();
            assert_eq!(ids, ["live", "registered"]);
        }
        pool.close().await;
    }
}
