//! Workspace provisioning without changing the legacy migration ledger.
//!
//! Transcript search owns its own package stream, [`SEARCH`], applied only
//! while the feature is on. A database created with search off records the
//! [`searchless_bootstrap`] as its workspace baseline and never needs the
//! trigram extensions.
use std::sync::LazyLock;

use cctui_server_core::schema::{Migration, MigrationConnection, Origin};
use sha2::{Digest, Sha256};
use sqlx::PgConnection;

/// The workspace baseline corresponding to legacy schema 191.
pub const BOOTSTRAP: &str = include_str!("../schema/0001_workspace.sql");

pub const SEARCH_PACKAGE: &str = "workspace_search";

pub const SEARCH: &[Migration] = &[Migration {
    version: 1,
    name: "transcript_search",
    sql: include_str!("../schema/0007_transcript_search.sql"),
}];

const SEARCH_BLOCKS: [&str; 4] = [
    "-- EXTENSION: btree_gin",
    "-- EXTENSION: pg_trgm",
    "-- INDEX: idx_search_chunks_session_body_trgm",
    "-- INDEX: idx_stream_events_session_search_trgm",
];

static SEARCHLESS: LazyLock<String> = LazyLock::new(|| without_blocks(BOOTSTRAP, &SEARCH_BLOCKS));

/// Immutable like an applied migration: a searchless database records its checksum.
#[must_use]
pub fn searchless_bootstrap() -> &'static str {
    SEARCHLESS.as_str()
}

fn without_blocks(sql: &str, headers: &[&str]) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut skipping = false;
    for line in sql.split_inclusive('\n') {
        if skipping {
            skipping = line != "\n";
        } else if headers.contains(&line.trim_end_matches('\n')) {
            skipping = true;
        } else {
            out.push_str(line);
        }
    }
    out
}

pub const MIGRATIONS: &[Migration] = &[
    Migration { version: 1, name: "workspace", sql: BOOTSTRAP },
    Migration {
        version: 2,
        name: "dispatch_identities",
        sql: include_str!("../schema/0002_dispatch_identities.sql"),
    },
    Migration {
        version: 3,
        name: "scheduled_message_sealed_body",
        sql: include_str!("../schema/0003_scheduled_message_sealed_body.sql"),
    },
    Migration {
        version: 4,
        name: "user_hidden_owners",
        sql: include_str!("../schema/0004_user_hidden_owners.sql"),
    },
    Migration {
        version: 5,
        name: "search_chunks_gin_fastupdate_off",
        sql: include_str!("../schema/0005_search_chunks_gin_fastupdate_off.sql"),
    },
    Migration {
        version: 6,
        name: "stream_events_search_text_on_insert",
        sql: include_str!("../schema/0006_stream_events_search_text_on_insert.sql"),
    },
];

/// Expand a core installation, or adopt the already migrated full schema.
pub async fn provision(conn: &mut MigrationConnection, search: bool) -> Result<(), sqlx::Error> {
    let adopted = cctui_server_core::schema::origin(conn).await? == Some(Origin::LegacyFull);
    let mut migrations = MIGRATIONS.to_vec();
    migrations[0] = baseline(conn, search || adopted).await?;
    if core_created_dispatch_identities(conn).await? {
        cctui_server_core::schema::install_package(conn, "workspace", &migrations[..1], adopted)
            .await?;
        cctui_server_core::schema::adopt(conn, "workspace", migrations[1]).await?;
    }
    cctui_server_core::schema::install_package(conn, "workspace", &migrations, adopted).await?;
    if search {
        cctui_server_core::schema::install_package(conn, SEARCH_PACKAGE, SEARCH, false).await?;
    }
    Ok(())
}

async fn baseline(conn: &mut PgConnection, full: bool) -> Result<Migration, sqlx::Error> {
    let recorded: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT checksum FROM _cctui_schema_migrations WHERE package = 'workspace' AND version = 1",
    )
    .fetch_optional(conn)
    .await?;
    let searchless = recorded.map_or(!full, |checksum| {
        checksum == Sha256::digest(searchless_bootstrap().as_bytes()).to_vec()
    });
    Ok(if searchless {
        Migration { version: 1, name: "workspace", sql: searchless_bootstrap() }
    } else {
        MIGRATIONS[0]
    })
}

/// Core migration five creates the table workspace migration two would create again.
async fn core_created_dispatch_identities(conn: &mut PgConnection) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT to_regclass('public.dispatch_identities') IS NOT NULL \
         AND (SELECT count(*) FROM _cctui_schema_migrations WHERE package = 'workspace') < 2",
    )
    .fetch_one(conn)
    .await
}

#[cfg(test)]
mod tests {
    use cctui_server_core::test_support::with_isolated_database;
    use sha2::{Digest, Sha256};

    #[test]
    fn no_statement_outside_the_search_step_needs_a_search_extension() {
        let outside = std::iter::once(super::searchless_bootstrap())
            .chain(super::MIGRATIONS[1..].iter().map(|m| m.sql))
            .chain(cctui_server_core::schema::MIGRATIONS.iter().map(|m| m.sql));
        for sql in outside {
            let sql = sql.to_ascii_lowercase();
            for needle in ["pg_trgm", "btree_gin", "gin_trgm_ops", "gist_trgm_ops"] {
                assert!(!sql.contains(needle), "{needle} outside the search step");
            }
        }
        for needle in ["pg_trgm", "btree_gin", "gin_trgm_ops"] {
            assert!(super::SEARCH[0].sql.contains(needle), "{needle}");
        }
    }

    #[test]
    fn the_searchless_baseline_drops_exactly_the_search_blocks() {
        let (full, searchless) = (super::BOOTSTRAP, super::searchless_bootstrap());
        for header in super::SEARCH_BLOCKS {
            assert_eq!(full.lines().filter(|l| *l == header).count(), 1, "{header}");
            assert!(!searchless.contains(header), "{header}");
        }
        assert_eq!(
            full.lines().count() - searchless.lines().count(),
            3 * super::SEARCH_BLOCKS.len()
        );
        assert_eq!(
            hex::encode(Sha256::digest(searchless.as_bytes())),
            "9f6fa27a551e92c65460576688321549f20fc41e480c2332284d51082a021488",
            "a searchless database records this checksum"
        );
    }

    async fn provisioned(pool: &sqlx::PgPool) {
        cctui_server_core::schema::bootstrap(pool).await.unwrap();
        let mut conn = cctui_server_core::schema::migration_connection(pool).await.unwrap();
        super::provision(&mut conn, true).await.unwrap();
    }

    async fn installations(pool: &sqlx::PgPool, package: &str) -> Vec<(i64, String)> {
        sqlx::query_as(
            "SELECT version, installation FROM _cctui_schema_migrations WHERE package = $1 \
             ORDER BY version",
        )
        .bind(package)
        .fetch_all(pool)
        .await
        .unwrap()
    }

    async fn insert_identity(pool: &sqlx::PgPool, id: uuid::Uuid) -> u64 {
        sqlx::query(
            "INSERT INTO dispatch_identities (dispatch_id, dispatcher_id, user_id, \
             session_id, namespace, job_name, machine_key_hash, expires_at) \
             VALUES ($1, $1, $1, 's', 'ns', 'job', 'h', now())",
        )
        .bind(id)
        .execute(pool)
        .await
        .unwrap()
        .rows_affected()
    }

    #[tokio::test]
    async fn fresh_full_provisioning_adopts_the_core_dispatch_identity_table() {
        with_isolated_database("dispatch_identity_migration", |pool| async move {
            provisioned(&pool).await;
            let present: bool =
                sqlx::query_scalar("SELECT to_regclass('public.dispatch_identities') IS NOT NULL")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert!(present);
            let mut conn = cctui_server_core::schema::migration_connection(&pool).await.unwrap();
            super::provision(&mut conn, true).await.unwrap();
            drop(conn);
            assert_eq!(
                installations(&pool, "workspace").await,
                [
                    (1, "bootstrap".to_owned()),
                    (2, "adopted".to_owned()),
                    (3, "bootstrap".to_owned()),
                    (4, "bootstrap".to_owned()),
                    (5, "bootstrap".to_owned()),
                    (6, "bootstrap".to_owned())
                ]
            );

            sqlx::raw_sql("DROP TABLE public.dispatch_identities").execute(&pool).await.unwrap();
            sqlx::raw_sql(super::MIGRATIONS[1].sql).execute(&pool).await.unwrap();
            let inserted = insert_identity(&pool, uuid::Uuid::new_v4()).await;
            assert_eq!(inserted, 1, "re-applied up works");
        })
        .await;
    }

    #[tokio::test]
    async fn search_chunk_inserts_bypass_the_gin_pending_list() {
        with_isolated_database("search_chunks_fastupdate", |pool| async move {
            provisioned(&pool).await;
            let options: Vec<String> = sqlx::query_scalar(
                "SELECT unnest(reloptions) FROM pg_class \
                 WHERE oid = 'public.idx_search_chunks_session_body_trgm'::regclass",
            )
            .fetch_all(&pool)
            .await
            .unwrap();
            assert!(options.iter().any(|o| o == "fastupdate=off"), "{options:?}");
        })
        .await;
    }

    #[tokio::test]
    async fn search_text_keeps_the_former_generated_value_unless_given() {
        with_isolated_database("search_text_on_insert", |pool| async move {
            provisioned(&pool).await;
            sqlx::raw_sql(
                "INSERT INTO sessions (id, machine_id, working_dir) VALUES ('s', 'm', '/w')",
            )
            .execute(&pool)
            .await
            .unwrap();
            let payloads = [
                serde_json::json!({ "text": "hello" }),
                serde_json::json!({ "content": "out", "tool": "Bash", "input": { "cmd": "ls" } }),
                serde_json::json!({ "content": [{ "type": "text", "text": "x" }] }),
                serde_json::json!({ "input": null, "text": null }),
                serde_json::json!({}),
            ];
            for payload in &payloads {
                let (stored, former): (String, String) = sqlx::query_as(
                    "INSERT INTO stream_events (session_id, event_type, payload) \
                     VALUES ('s', 'tool_use', $1) \
                     RETURNING search_text, \
                       COALESCE(payload ->> 'text', '') || ' ' \
                       || COALESCE(payload ->> 'content', '') || ' ' \
                       || COALESCE(payload ->> 'tool', '') || ' ' \
                       || COALESCE((payload -> 'input')::text, '')",
                )
                .bind(payload)
                .fetch_one(&pool)
                .await
                .unwrap();
                assert_eq!(stored, former, "{payload}");
            }
            let given: String = sqlx::query_scalar(
                "INSERT INTO stream_events (session_id, event_type, payload, search_text) \
                 VALUES ('s', 'message', '{\"text\": \"t\"}', '') RETURNING search_text",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(given, "");
        })
        .await;
    }

    #[tokio::test]
    async fn a_database_with_workspace_two_installed_takes_core_five_cleanly() {
        with_isolated_database("dispatch_identity_preexisting", |pool| async move {
            provisioned(&pool).await;
            sqlx::raw_sql(
                "DELETE FROM _cctui_schema_migrations WHERE package = 'core' AND version >= 5; \
                 UPDATE _cctui_schema_migrations SET installation = 'bootstrap' \
                 WHERE package = 'workspace' AND version = 2",
            )
            .execute(&pool)
            .await
            .unwrap();
            let kept = uuid::Uuid::new_v4();
            assert_eq!(insert_identity(&pool, kept).await, 1);

            provisioned(&pool).await;
            let core: Vec<i64> =
                installations(&pool, "core").await.into_iter().map(|(v, _)| v).collect();
            assert_eq!(core, [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14]);
            assert_eq!(
                installations(&pool, "workspace").await,
                [
                    (1, "bootstrap".to_owned()),
                    (2, "bootstrap".to_owned()),
                    (3, "bootstrap".to_owned()),
                    (4, "bootstrap".to_owned()),
                    (5, "bootstrap".to_owned()),
                    (6, "bootstrap".to_owned())
                ]
            );
            let rows: i64 =
                sqlx::query_scalar("SELECT count(*) FROM dispatch_identities WHERE dispatch_id=$1")
                    .bind(kept)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(rows, 1);
        })
        .await;
    }
}
