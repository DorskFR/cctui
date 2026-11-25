//! Shared coordination of package-owned schema changes.
use sha2::{Digest, Sha256};
use sqlx::{Connection, PgConnection, PgPool};
use std::ops::{Deref, DerefMut};
use std::time::{Duration, Instant};

pub mod core;

/// Last migration in the historical, shared migration stream.
pub const LEGACY_BASELINE: i64 = 191;
/// One immutable, numbered package migration. Version one is its baseline.
#[derive(Debug, Clone, Copy)]
pub struct Migration {
    pub version: i64,
    pub name: &'static str,
    pub sql: &'static str,
}

const LOCK_KEY: i64 = 0x6363_7475_695f_6d67;
pub const TRACKING: &str = "\
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

pub fn incompatible(message: impl Into<String>) -> sqlx::Error {
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

pub async fn installed(
    conn: &mut PgConnection,
    package: &str,
    sql: &str,
) -> Result<bool, sqlx::Error> {
    let checksum: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT checksum FROM _cctui_schema_migrations WHERE package=$1 AND version=1",
    )
    .bind(package)
    .fetch_optional(conn)
    .await?;
    match checksum {
        None => Ok(false),
        Some(checksum) if checksum == Sha256::digest(sql.as_bytes()).to_vec() => Ok(true),
        Some(_) => Err(incompatible(format!("altered {package} baseline"))),
    }
}

pub async fn record(
    conn: &mut PgConnection,
    package: &str,
    migration: Migration,
    adopted: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO _cctui_schema_migrations (package,version,checksum,installation) VALUES ($1,$2,$3,$4)"
    ).bind(package).bind(migration.version)
        .bind(Sha256::digest(migration.sql.as_bytes()).to_vec())
        .bind(if adopted { "adopted" } else { "bootstrap" })
        .execute(conn).await?;
    Ok(())
}

/// Validate immutable history and apply each missing migration transactionally.
///
/// Only the baseline can be adopted from legacy storage. Forward migrations
/// always execute their SQL and never alter the historical sqlx ledger.
pub async fn install_package(
    conn: &mut MigrationConnection,
    package: &str,
    migrations: &[Migration],
    adopted: bool,
) -> Result<(), sqlx::Error> {
    if migrations.is_empty()
        || migrations
            .iter()
            .enumerate()
            .any(|(i, m)| usize::try_from(m.version).ok() != Some(i + 1))
    {
        return Err(incompatible("package migrations must be contiguous from version one"));
    }
    let rows: Vec<(i64, Vec<u8>)> = sqlx::query_as(
        "SELECT version,checksum FROM _cctui_schema_migrations WHERE package=$1 ORDER BY version",
    )
    .bind(package)
    .fetch_all(&mut **conn)
    .await?;
    if rows.len() > migrations.len()
        || rows.iter().zip(migrations).any(|((version, checksum), migration)| {
            *version != migration.version
                || *checksum != Sha256::digest(migration.sql.as_bytes()).to_vec()
        })
    {
        return Err(incompatible(format!("unsupported or altered {package} schema migration")));
    }
    let pending = &migrations[rows.len()..];
    if pending.is_empty() {
        return Ok(());
    }
    let started = Instant::now();
    for migration in pending {
        let (version, name) = (migration.version, migration.name);
        let adopting = adopted && version == 1;
        if adopting {
            tracing::info!(package, version, name, "adopting existing migration {version} {name}");
        } else {
            tracing::info!(
                package,
                version,
                name,
                "applying migration {version} {name}; this can take a while on large tables"
            );
        }
        let applying = Instant::now();
        let mut tx = conn.begin().await?;
        if !adopting {
            sqlx::raw_sql(migration.sql).execute(&mut *tx).await?;
        }
        record(&mut tx, package, *migration, adopting).await?;
        tx.commit().await?;
        if !adopting {
            let elapsed = applying.elapsed();
            tracing::info!(
                package,
                version,
                name,
                ?elapsed,
                "applied migration {version} {name} in {elapsed:?}"
            );
        }
    }
    let elapsed = started.elapsed();
    tracing::info!(
        package,
        count = pending.len(),
        ?elapsed,
        "applied {} {package} migrations in {elapsed:?}",
        pending.len()
    );
    Ok(())
}

/// Record a package migration whose objects already exist, without running it.
pub async fn adopt(
    conn: &mut MigrationConnection,
    package: &str,
    migration: Migration,
) -> Result<(), sqlx::Error> {
    record(conn, package, migration, true).await
}
