use std::time::Duration;

use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

/// Read a `u32` env var, falling back to `default` when unset or unparseable.
fn env_u32(name: &str, default: u32) -> u32 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

pub async fn connect(database_url: &str) -> Result<PgPool, sqlx::Error> {
    // Pool sizing is env-tunable so prod can scale connections without a rebuild.
    // Defaults are generous enough to absorb gateway proxying + heartbeats +
    // dispatcher bumps + webui loads without starving the pool (CCT slow-pool fix);
    // acquire_timeout fails fast instead of hanging requests (and the UI) ~30s.
    let max_connections = env_u32("CCTUI_DB_MAX_CONNECTIONS", 40);
    let min_connections = env_u32("CCTUI_DB_MIN_CONNECTIONS", 5);
    let acquire_timeout = env_u32("CCTUI_DB_ACQUIRE_TIMEOUT_SECS", 5);

    let pool = PgPoolOptions::new()
        .max_connections(max_connections)
        .min_connections(min_connections)
        .acquire_timeout(Duration::from_secs(u64::from(acquire_timeout)))
        .connect(database_url)
        .await?;

    sqlx::migrate!("../../migrations").run(&pool).await?;

    tracing::info!(
        max_connections,
        min_connections,
        acquire_timeout_secs = acquire_timeout,
        "database connected and migrations applied"
    );
    Ok(pool)
}
