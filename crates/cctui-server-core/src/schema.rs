//! Core provisioning and shared coordination of package-owned schema changes.
use sha2::{Digest, Sha256, Sha384};
use sqlx::{Connection, PgConnection, PgPool};
use std::ops::{Deref, DerefMut};
use std::time::Duration;

/// Immutable execution baseline corresponding to legacy schema 191.
pub const BOOTSTRAP: &str = include_str!("../schema/0001_execution.sql");
/// Last migration in the historical, shared migration stream.
pub const LEGACY_BASELINE: i64 = 191;
const LEGACY_LAST: &str = include_str!("../../../migrations/191_file_blobs.up.sql");
const LOCK_KEY: i64 = 0x6363_7475_695f_6d67;
const TRACKING: &str = "\
    CREATE TABLE _cctui_schema_lineage ( \
        singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton), \
        origin text NOT NULL CHECK (origin IN ('core', 'legacy_full')), \
        legacy_baseline bigint NOT NULL); \
    CREATE TABLE _cctui_schema_migrations ( \
        package text NOT NULL, version bigint NOT NULL, checksum bytea NOT NULL, \
        installation text NOT NULL CHECK (installation IN ('bootstrap', 'adopted')), \
        installed_on timestamptz NOT NULL DEFAULT now(), PRIMARY KEY (package, version));";

/// The schema's original provisioning path, retained after core-to-full expansion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Core,
    LegacyFull,
}

/// Owns the database migration lock until its physical connection is dropped.
///
/// A detached connection cannot return an advisory lock to the pool after an
/// error or cancellation. The lock key matches the full server's existing lock.
pub struct MigrationConnection(PgConnection);
impl Deref for MigrationConnection {
    type Target = PgConnection;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl DerefMut for MigrationConnection {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// Serialize schema work without holding a waiting `PostgreSQL` snapshot.
pub async fn migration_connection(pool: &PgPool) -> Result<MigrationConnection, sqlx::Error> {
    let mut conn = pool.acquire().await?.detach();
    while !sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_lock($1)")
        .bind(LOCK_KEY)
        .fetch_one(&mut conn)
        .await?
    {
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Ok(MigrationConnection(conn))
}

fn incompatible(message: impl Into<String>) -> sqlx::Error {
    sqlx::Error::Protocol(message.into())
}

/// Read lineage without creating or changing any schema objects.
pub async fn origin(conn: &mut PgConnection) -> Result<Option<Origin>, sqlx::Error> {
    let tracked: bool =
        sqlx::query_scalar("SELECT to_regclass('_cctui_schema_lineage') IS NOT NULL")
            .fetch_one(&mut *conn)
            .await?;
    if !tracked {
        return Ok(None);
    }
    let row: (String, i64) =
        sqlx::query_as("SELECT origin, legacy_baseline FROM _cctui_schema_lineage WHERE singleton")
            .fetch_one(&mut *conn)
            .await?;
    if row.1 != LEGACY_BASELINE {
        return Err(incompatible("unsupported schema baseline"));
    }
    match row.0.as_str() {
        "core" => Ok(Some(Origin::Core)),
        "legacy_full" => Ok(Some(Origin::LegacyFull)),
        _ => Err(incompatible("unrecognized schema lineage")),
    }
}

async fn installed(conn: &mut PgConnection, package: &str, sql: &str) -> Result<bool, sqlx::Error> {
    let rows: Vec<(i64, Vec<u8>)> = sqlx::query_as(
        "SELECT version, checksum FROM _cctui_schema_migrations WHERE package=$1 ORDER BY version",
    )
    .bind(package)
    .fetch_all(conn)
    .await?;
    if rows.is_empty() {
        return Ok(false);
    }
    let checksum = Sha256::digest(sql.as_bytes()).to_vec();
    if rows.len() != 1 || rows[0].0 != 1 || rows[0].1 != checksum {
        return Err(incompatible(format!("unsupported or altered {package} schema migration")));
    }
    Ok(true)
}

async fn record(
    conn: &mut PgConnection,
    package: &str,
    sql: &str,
    adopted: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO _cctui_schema_migrations (package,version,checksum,installation) \
         VALUES ($1,1,$2,$3)",
    )
    .bind(package)
    .bind(Sha256::digest(sql.as_bytes()).to_vec())
    .bind(if adopted { "adopted" } else { "bootstrap" })
    .execute(conn)
    .await?;
    Ok(())
}

/// Apply a package baseline atomically, or record explicit legacy adoption.
///
/// Adoption records identify an existing baseline; they never claim its SQL
/// was executed and never insert records into the legacy sqlx ledger.
pub async fn install_package(
    conn: &mut MigrationConnection,
    package: &str,
    sql: &'static str,
    adopted: bool,
) -> Result<(), sqlx::Error> {
    if installed(conn, package, sql).await? {
        return Ok(());
    }
    let mut tx = conn.begin().await?;
    if !adopted {
        sqlx::raw_sql(sql).execute(&mut *tx).await?;
    }
    record(&mut tx, package, sql, adopted).await?;
    tx.commit().await
}

async fn verify_legacy(conn: &mut PgConnection) -> Result<(), sqlx::Error> {
    let dirty: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM _sqlx_migrations WHERE NOT success)")
            .fetch_one(&mut *conn)
            .await?;
    if dirty {
        return Err(incompatible("legacy schema has an unfinished migration"));
    }
    let row: Option<(i64, Vec<u8>, bool)> =
        sqlx::query_as("SELECT version,checksum,success FROM _sqlx_migrations WHERE version=$1")
            .bind(LEGACY_BASELINE)
            .fetch_optional(&mut *conn)
            .await?;
    let expected = Sha384::digest(LEGACY_LAST.as_bytes()).to_vec();
    if !matches!(row, Some((LEGACY_BASELINE, checksum, true)) if checksum == expected) {
        return Err(incompatible("core requires the supported legacy schema baseline"));
    }
    let names: Vec<&str> = BOOTSTRAP
        .lines()
        .filter_map(|line| line.strip_prefix("CREATE TABLE public."))
        .filter_map(|line| line.strip_suffix(" ("))
        .collect();
    let missing: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM unnest($1::text[]) AS name WHERE to_regclass(name) IS NULL",
    )
    .bind(names)
    .fetch_all(conn)
    .await?;
    if !missing.is_empty() {
        return Err(incompatible(format!("legacy core schema is missing tables: {missing:?}")));
    }
    Ok(())
}

/// Provision an empty core database or adopt a supported legacy full database.
pub async fn provision(conn: &mut MigrationConnection) -> Result<(), sqlx::Error> {
    if let Some(origin) = origin(conn).await? {
        if origin == Origin::LegacyFull {
            verify_legacy(conn).await?;
        }
        if !installed(conn, "core", BOOTSTRAP).await? {
            return Err(incompatible("core schema tracking is incomplete"));
        }
        return Ok(());
    }
    let legacy: bool = sqlx::query_scalar("SELECT to_regclass('_sqlx_migrations') IS NOT NULL")
        .fetch_one(&mut **conn)
        .await?;
    if legacy {
        verify_legacy(conn).await?;
    } else {
        let empty: bool = sqlx::query_scalar(
            "SELECT current_schema()='public' AND NOT EXISTS ( \
                SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace \
                WHERE n.nspname='public' AND c.relkind IN ('r','p','v','m','S'))",
        )
        .fetch_one(&mut **conn)
        .await?;
        if !empty {
            return Err(incompatible("fresh core provisioning requires an empty public schema"));
        }
    }
    let mut tx = conn.begin().await?;
    sqlx::raw_sql(TRACKING).execute(&mut *tx).await?;
    if !legacy {
        sqlx::raw_sql(BOOTSTRAP).execute(&mut *tx).await?;
    }
    sqlx::query("INSERT INTO _cctui_schema_lineage (origin,legacy_baseline) VALUES ($1,$2)")
        .bind(if legacy { "legacy_full" } else { "core" })
        .bind(LEGACY_BASELINE)
        .execute(&mut *tx)
        .await?;
    record(&mut tx, "core", BOOTSTRAP, legacy).await?;
    tx.commit().await
}

/// Core startup entry point; all changes share the full host's coordination lock.
pub async fn bootstrap(pool: &PgPool) -> Result<(), sqlx::Error> {
    let mut conn = migration_connection(pool).await?;
    provision(&mut conn).await
}

#[cfg(test)]
mod tests;
