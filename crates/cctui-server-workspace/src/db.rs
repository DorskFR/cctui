use std::str::FromStr;
use std::time::Duration;

use cctui_proto::features::{Feature, FeatureSet};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{ConnectOptions, PgPool};

/// The embedded migrations. A `static` keeps the list off the stack: built
/// inline in `connect`, it outgrew clippy's `large_stack_arrays` limit.
/// Migrations applied by a newer release are tolerated, so rolling back to this
/// binary doesn't need a schema rollback first.
static MIGRATOR: sqlx::migrate::Migrator =
    sqlx::migrate::Migrator { ignore_missing: true, ..sqlx::migrate!("../../migrations") };

#[cfg(test)]
#[path = "db_split_tests.rs"]
mod split_tests;

/// Read a `u32` env var, falling back to `default` when unset or unparseable.
fn env_u32(name: &str, default: u32) -> u32 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

#[must_use]
pub fn slow_query_ms() -> u32 {
    env_u32("CCTUI_DB_SLOW_QUERY_MS", 500)
}

/// `database_url` with statements slower than `CCTUI_DB_SLOW_QUERY_MS` logged
/// at warn.
pub fn connect_options(database_url: &str) -> Result<PgConnectOptions, sqlx::Error> {
    connect_options_with(database_url, slow_query_ms())
}

fn connect_options_with(database_url: &str, slow_ms: u32) -> Result<PgConnectOptions, sqlx::Error> {
    let options = PgConnectOptions::from_str(database_url)?;
    let threshold = Duration::from_millis(u64::from(slow_ms));
    Ok(options.log_slow_statements(log::LevelFilter::Warn, threshold))
}

/// Acquiring skips sqlx's liveness ping, a full round trip per statement: a
/// dead connection fails its query and is dropped, and the idle and lifetime
/// caps recycle half-open sockets.
fn pool_options(max_connections: u32, min_connections: u32, acquire_secs: u32) -> PgPoolOptions {
    let secs = |name, default| Duration::from_secs(u64::from(env_u32(name, default)));
    PgPoolOptions::new()
        .max_connections(max_connections)
        .min_connections(min_connections)
        .acquire_timeout(Duration::from_secs(u64::from(acquire_secs)))
        .test_before_acquire(false)
        .idle_timeout(secs("CCTUI_DB_IDLE_TIMEOUT_SECS", 300))
        .max_lifetime(secs("CCTUI_DB_MAX_LIFETIME_SECS", 1800))
}

#[cfg(test)]
pub async fn connect(database_url: &str) -> Result<PgPool, sqlx::Error> {
    connect_reporting(database_url).await.map(|(pool, _)| pool)
}

/// [`connect_reporting_for`] with every feature on.
pub async fn connect_reporting(database_url: &str) -> Result<(PgPool, Vec<i64>), sqlx::Error> {
    connect_reporting_for(database_url, FeatureSet::all()).await
}

/// [`connect`], also returning the migration versions this call applied. With
/// `transcript_search` off, a fresh database never needs the trigram extensions.
pub async fn connect_reporting_for(
    database_url: &str,
    features: FeatureSet,
) -> Result<(PgPool, Vec<i64>), sqlx::Error> {
    // Pool sizing is env-tunable without a rebuild. The default keeps two
    // replicas plus a rolling-update surge (4 pods) under Postgres' default
    // max_connections of 100; acquire_timeout fails fast instead of hanging requests.
    let max_connections = env_u32("CCTUI_DB_MAX_CONNECTIONS", 20);
    let min_connections = env_u32("CCTUI_DB_MIN_CONNECTIONS", 5);
    let acquire_timeout = env_u32("CCTUI_DB_ACQUIRE_TIMEOUT_SECS", 5);
    let slow_query_ms = slow_query_ms();
    let connect_options = connect_options_with(database_url, slow_query_ms)?;

    let pool = pool_options(max_connections, min_connections, acquire_timeout)
        .connect_with(connect_options)
        .await?;

    let applied = migrate(&pool, features.contains(Feature::TranscriptSearch)).await?;

    tracing::info!(
        max_connections,
        min_connections,
        acquire_timeout_secs = acquire_timeout,
        slow_query_ms,
        applied = ?applied,
        "database connected and migrations applied"
    );
    Ok((pool, applied))
}

/// Serializes migrations across replicas with a polled advisory lock. A
/// blocking `pg_advisory_lock` waiter holds a snapshot that a concurrent
/// `CREATE INDEX CONCURRENTLY` must wait out, which deadlocks the two.
///
/// The legacy stream creates the search extensions, so without `search` an
/// empty database is provisioned through the package streams instead.
async fn migrate(pool: &PgPool, search: bool) -> Result<Vec<i64>, sqlx::Error> {
    let mut conn = cctui_server_core::schema::migration_connection(pool).await?;
    if cctui_server_core::schema::origin(&mut conn).await?
        == Some(cctui_server_core::schema::Origin::Core)
        || (!search && is_empty(&mut conn).await?)
    {
        cctui_server_core::schema::provision(&mut conn).await?;
        crate::schema::provision(&mut conn, search).await?;
        return Ok(Vec::new());
    }
    reconcile_migration_checksums(pool).await?;
    let before = applied_versions(&mut conn).await?;
    MIGRATOR.run(&mut *conn).await?;
    let after = applied_versions(&mut conn).await?;
    cctui_server_core::schema::provision(&mut conn).await?;
    crate::schema::provision(&mut conn, search).await?;
    Ok(after.into_iter().filter(|v| !before.contains(v)).collect())
}

async fn is_empty(conn: &mut sqlx::PgConnection) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT NOT EXISTS (SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = 'public' AND c.relkind IN ('r', 'p', 'v', 'm', 'S'))",
    )
    .fetch_one(conn)
    .await
}

async fn applied_versions(conn: &mut sqlx::PgConnection) -> Result<Vec<i64>, sqlx::Error> {
    let exists: bool = sqlx::query_scalar("SELECT to_regclass('_sqlx_migrations') IS NOT NULL")
        .fetch_one(&mut *conn)
        .await?;
    if !exists {
        return Ok(Vec::new());
    }
    sqlx::query_scalar("SELECT version FROM _sqlx_migrations WHERE success ORDER BY version")
        .fetch_all(conn)
        .await
}

async fn reconcile_migration_checksums(pool: &PgPool) -> Result<(), sqlx::Error> {
    const RECONCILED: &[(i64, &str)] = &[
        (
            51,
            "41e5695b0fb3238b1d8ecc979de99c2daff7289615cbf8b08c68f9c9d5b9b4b8bfb3bc2d2183f339fa4f2a6c0aa508ad",
        ),
        (
            55,
            "c23c56dd3c04b56e6874526fd3a43f783e9ff844ccff98e8531d984628725bbf2e1014a19df3204234b3079345a6d177",
        ),
        (
            60,
            "8d0caf988684f0c4894fe6ea43ec74c44453e79c23a68518604e77e78e9a57466d792b8e6253090b80eea4c278d400dd",
        ),
        (
            143,
            "6b54d6a5cf2c7c5e3d701fbd49a6a45818bb10bc0032bbd811f840ebbd809322fabcb1b28b0fec596e6a4d362e37c4e9",
        ),
    ];
    let table_exists: bool =
        sqlx::query_scalar("SELECT to_regclass('_sqlx_migrations') IS NOT NULL")
            .fetch_one(pool)
            .await?;
    if !table_exists {
        return Ok(());
    }

    for (version, checksum_hex) in RECONCILED {
        let checksum = hex::decode(checksum_hex).expect("static checksum hex is valid");
        sqlx::query("UPDATE _sqlx_migrations SET checksum = $1 WHERE version = $2")
            .bind(checksum)
            .bind(version)
            .execute(pool)
            .await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pool_acquires_without_a_ping_and_recycles_idle_connections() {
        let options = pool_options(20, 5, 5);
        assert!(!options.get_test_before_acquire());
        assert_eq!(options.get_max_connections(), 20);
        assert_eq!(options.get_min_connections(), 5);
        assert_eq!(options.get_acquire_timeout(), Duration::from_secs(5));
        let idle = options.get_idle_timeout().expect("idle timeout set");
        let lifetime = options.get_max_lifetime().expect("max lifetime set");
        assert!(idle <= lifetime, "{idle:?} > {lifetime:?}");
    }

    #[test]
    fn connect_options_log_statements_slower_than_the_threshold() {
        let options = connect_options_with("postgres://u:p@db/app", 1234).unwrap();
        let shown = format!("{options:?}");
        assert!(shown.contains("slow_statements_level: Warn"), "{shown}");
        assert!(shown.contains("slow_statements_duration: 1.234s"), "{shown}");
        assert_eq!(options.get_host(), "db");
    }

    #[tokio::test]
    async fn a_migration_from_a_newer_release_does_not_block_startup() {
        let Some(url) = crate::routes::gateway::test_db_url("db_newer_migration") else {
            return;
        };
        let pool = connect(&url).await.expect("connect");
        let version = 9_000_000_000_i64 + i64::from(std::process::id());
        sqlx::query(
            "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) \
             VALUES ($1, 'from a newer release', true, '\\x00'::bytea, 0)",
        )
        .bind(version)
        .execute(&pool)
        .await
        .expect("insert future migration");
        let result = migrate(&pool, true).await;
        sqlx::query("DELETE FROM _sqlx_migrations WHERE version = $1")
            .bind(version)
            .execute(&pool)
            .await
            .expect("cleanup");
        assert!(result.is_ok(), "{result:?}");
    }
}
