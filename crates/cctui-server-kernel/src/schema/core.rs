//! Core provisioning of the base package schema stream.
use sha2::{Digest, Sha384};
use sqlx::{Connection, PgConnection, PgPool};

use super::{
    LEGACY_BASELINE, Migration, MigrationConnection, Origin, TRACKING, incompatible,
    install_package, installed, migration_connection, origin, record,
};

/// Immutable execution baseline corresponding to legacy schema 191.
pub const BOOTSTRAP: &str = include_str!("../../schema/core/0001_execution.sql");
pub const MIGRATIONS: &[Migration] = &[
    Migration { version: 1, name: "execution", sql: BOOTSTRAP },
    Migration {
        version: 2,
        name: "machine_launches",
        sql: include_str!("../../schema/core/0002_machine_launches.sql"),
    },
    Migration {
        version: 3,
        name: "execution_stops",
        sql: include_str!("../../schema/core/0003_execution_stops.sql"),
    },
    Migration {
        version: 4,
        name: "session_name_from_custom_title",
        sql: include_str!("../../schema/core/0004_session_name_from_custom_title.sql"),
    },
    Migration {
        version: 5,
        name: "dispatch_identities",
        sql: include_str!("../../schema/core/0005_dispatch_identities.sql"),
    },
    Migration {
        version: 6,
        name: "session_tool_guard_bypass",
        sql: include_str!("../../schema/core/0006_session_tool_guard_bypass.sql"),
    },
    Migration {
        version: 7,
        name: "account_hook_policy",
        sql: include_str!("../../schema/core/0007_account_hook_policy.sql"),
    },
    Migration {
        version: 8,
        name: "session_hook_policies",
        sql: include_str!("../../schema/core/0008_session_hook_policies.sql"),
    },
    Migration {
        version: 9,
        name: "dispatch_exposure",
        sql: include_str!("../../schema/core/0009_dispatch_exposure.sql"),
    },
    Migration {
        version: 10,
        name: "archived_claude_jobs_index",
        sql: include_str!("../../schema/core/0010_archived_claude_jobs_index.sql"),
    },
    Migration {
        version: 11,
        name: "provider_usage_totals",
        sql: include_str!("../../schema/core/0011_provider_usage_totals.sql"),
    },
    Migration {
        version: 12,
        name: "dispatch_handle_gone",
        sql: include_str!("../../schema/core/0012_dispatch_handle_gone.sql"),
    },
    Migration {
        version: 13,
        name: "dispatch_identity_hold",
        sql: include_str!("../../schema/core/0013_dispatch_identity_hold.sql"),
    },
    Migration {
        version: 14,
        name: "dispatcher_features",
        sql: include_str!("../../schema/core/0014_dispatcher_features.sql"),
    },
    Migration {
        version: 15,
        name: "oauth_pending_logins",
        sql: include_str!("../../schema/core/0015_oauth_pending_logins.sql"),
    },
];

const LEGACY_LAST: &str = include_str!("../../../../migrations/191_file_blobs.up.sql");

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
        return install_package(conn, "core", MIGRATIONS, false).await;
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
    record(&mut tx, "core", MIGRATIONS[0], legacy).await?;
    tx.commit().await?;
    install_package(conn, "core", MIGRATIONS, false).await
}

/// Core startup entry point; all changes share the full host's coordination lock.
pub async fn bootstrap(pool: &PgPool) -> Result<(), sqlx::Error> {
    let mut conn = migration_connection(pool).await?;
    provision(&mut conn).await
}
