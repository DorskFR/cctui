//! Workspace provisioning without changing the legacy migration ledger.
//!
//! Transcript search owns its own package stream, [`SEARCH`], applied only
//! while the feature is on. A database created with search off records the
//! [`searchless_bootstrap`] as its workspace baseline and never needs the
//! trigram extensions.
use cctui_server_search::schema::{SEARCH, SEARCH_PACKAGE};
use std::sync::LazyLock;

use cctui_server_kernel::schema::{Migration, MigrationConnection, Origin};
use sha2::{Digest, Sha256};
use sqlx::PgConnection;

/// The workspace baseline corresponding to legacy schema 191.
pub const BOOTSTRAP: &str = include_str!("../schema/0001_workspace.sql");

const SEARCH_BLOCKS: [&str; 4] = [
    "-- EXTENSION: btree_gin",
    "-- EXTENSION: pg_trgm",
    "-- INDEX: idx_search_chunks_session_body_trgm",
    "-- INDEX: idx_stream_events_session_search_trgm",
];

static SEARCHLESS: LazyLock<String> = LazyLock::new(|| without_blocks(BOOTSTRAP, &SEARCH_BLOCKS));

const SEARCH_TEXT_GIN_GUARD: &str =
    "IF pg_catalog.to_regclass('public.idx_stream_events_session_search_trgm') IS NULL THEN";

/// Search migration one without its index on the column workspace 31 drops,
/// for a database that has not recorded the original.
static SEARCH_BASELINE: LazyLock<String> =
    LazyLock::new(|| without_if_block(SEARCH[0].sql, SEARCH_TEXT_GIN_GUARD));

fn without_if_block(sql: &str, guard: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut skipping = false;
    for line in sql.split_inclusive('\n') {
        if skipping {
            skipping = line.trim() != "END IF;";
        } else if line.trim() == guard {
            skipping = true;
        } else {
            out.push_str(line);
        }
    }
    out
}

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
    Migration {
        version: 7,
        name: "notifications",
        sql: include_str!("../schema/0007_notifications.sql"),
    },
    Migration { version: 8, name: "inbox", sql: include_str!("../schema/0008_inbox.sql") },
    Migration {
        version: 9,
        name: "session_outcomes",
        sql: include_str!("../schema/0009_session_outcomes.sql"),
    },
    Migration {
        version: 10,
        name: "queued_sessions",
        sql: include_str!("../schema/0010_queued_sessions.sql"),
    },
    Migration { version: 11, name: "presets", sql: include_str!("../schema/0011_presets.sql") },
    Migration {
        version: 12,
        name: "inbox_preset_lanes",
        sql: include_str!("../schema/0012_inbox_preset_lanes.sql"),
    },
    Migration {
        version: 13,
        name: "timers_and_hops",
        sql: include_str!("../schema/0013_timers_and_hops.sql"),
    },
    Migration {
        version: 14,
        name: "fold_outboxes",
        sql: include_str!("../schema/0014_fold_outboxes.sql"),
    },
    Migration {
        version: 15,
        name: "env_endpoint_owner_only",
        sql: include_str!("../schema/0015_env_endpoint_owner_only.sql"),
    },
    Migration {
        version: 16,
        name: "drop_outboxes",
        sql: include_str!("../schema/0016_drop_outboxes.sql"),
    },
    Migration {
        version: 17,
        name: "search_text_without_base64",
        sql: include_str!("../schema/0017_search_text_without_base64.sql"),
    },
    Migration {
        version: 18,
        name: "storage_lifecycle",
        sql: include_str!("../schema/0018_storage_lifecycle.sql"),
    },
    Migration {
        version: 19,
        name: "redteam_runs",
        sql: include_str!("../schema/0019_redteam_runs.sql"),
    },
    Migration {
        version: 20,
        name: "preset_schedules",
        sql: include_str!("../schema/0020_preset_schedules.sql"),
    },
    Migration {
        version: 21,
        name: "preset_hooks",
        sql: include_str!("../schema/0021_preset_hooks.sql"),
    },
    Migration {
        version: 22,
        name: "preset_accounts",
        sql: include_str!("../schema/0022_preset_accounts.sql"),
    },
    Migration {
        version: 23,
        name: "preset_env",
        sql: include_str!("../schema/0023_preset_env.sql"),
    },
    Migration {
        version: 24,
        name: "preset_hook_budget",
        sql: include_str!("../schema/0024_preset_hook_budget.sql"),
    },
    Migration {
        version: 25,
        name: "notification_anchor",
        sql: include_str!("../schema/0025_notification_anchor.sql"),
    },
    Migration {
        version: 26,
        name: "preset_template",
        sql: include_str!("../schema/0026_preset_template.sql"),
    },
    Migration {
        version: 27,
        name: "session_thread_root",
        sql: include_str!("../schema/0027_session_thread_root.sql"),
    },
    Migration {
        version: 28,
        name: "session_followup_ops",
        sql: include_str!("../schema/0028_session_followup_ops.sql"),
    },
    Migration {
        version: 29,
        name: "profile_auto_followup",
        sql: include_str!("../schema/0029_profile_auto_followup.sql"),
    },
    Migration {
        version: 30,
        name: "retire_search_text_trigger",
        sql: include_str!("../schema/0030_retire_search_text_trigger.sql"),
    },
    Migration {
        version: 31,
        name: "drop_stream_events_search_text",
        sql: include_str!("../schema/0031_drop_stream_events_search_text.sql"),
    },
    Migration {
        version: 32,
        name: "context_reach_foreign_machines",
        sql: include_str!("../schema/0032_context_reach_foreign_machines.sql"),
    },
    Migration {
        version: 33,
        name: "privacy_scan_local_transcripts",
        sql: include_str!("../schema/0033_privacy_scan_local_transcripts.sql"),
    },
];

/// Expand a core installation, or adopt the already migrated full schema.
pub async fn provision(conn: &mut MigrationConnection, search: bool) -> Result<(), sqlx::Error> {
    let adopted = cctui_server_kernel::schema::origin(conn).await? == Some(Origin::LegacyFull);
    let mut migrations = MIGRATIONS.to_vec();
    migrations[0] = baseline(conn, search || adopted).await?;
    if core_created_dispatch_identities(conn).await? {
        cctui_server_kernel::schema::install_package(conn, "workspace", &migrations[..1], adopted)
            .await?;
        cctui_server_kernel::schema::adopt(conn, "workspace", migrations[1]).await?;
    }
    cctui_server_kernel::schema::install_package(conn, "workspace", &migrations, adopted).await?;
    if search {
        let stream = search_stream(conn).await?;
        cctui_server_kernel::schema::install_package(conn, SEARCH_PACKAGE, &stream, false).await?;
    }
    Ok(())
}

async fn search_stream(conn: &mut PgConnection) -> Result<Vec<Migration>, sqlx::Error> {
    let recorded: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT checksum FROM _cctui_schema_migrations WHERE package = $1 AND version = 1",
    )
    .bind(SEARCH_PACKAGE)
    .fetch_optional(conn)
    .await?;
    let mut stream = SEARCH.to_vec();
    let original = Sha256::digest(SEARCH[0].sql.as_bytes()).to_vec();
    if recorded.is_none_or(|checksum| checksum != original) {
        stream[0] = Migration { sql: SEARCH_BASELINE.as_str(), ..SEARCH[0] };
    }
    Ok(stream)
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
        Migration { sql: searchless_bootstrap(), ..MIGRATIONS[0] }
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
    use cctui_server_kernel::test_support::with_isolated_database;
    use sha2::{Digest, Sha256};

    #[test]
    fn no_statement_outside_the_search_step_needs_a_search_extension() {
        let outside = std::iter::once(super::searchless_bootstrap())
            .chain(super::MIGRATIONS[1..].iter().map(|m| m.sql))
            .chain(cctui_server_kernel::schema::core::MIGRATIONS.iter().map(|m| m.sql));
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
        cctui_server_kernel::schema::core::bootstrap(pool).await.unwrap();
        let mut conn = cctui_server_kernel::schema::migration_connection(pool).await.unwrap();
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
            let mut conn = cctui_server_kernel::schema::migration_connection(&pool).await.unwrap();
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
                    (6, "bootstrap".to_owned()),
                    (7, "bootstrap".to_owned()),
                    (8, "bootstrap".to_owned()),
                    (9, "bootstrap".to_owned()),
                    (10, "bootstrap".to_owned()),
                    (11, "bootstrap".to_owned()),
                    (12, "bootstrap".to_owned()),
                    (13, "bootstrap".to_owned()),
                    (14, "bootstrap".to_owned()),
                    (15, "bootstrap".to_owned()),
                    (16, "bootstrap".to_owned()),
                    (17, "bootstrap".to_owned()),
                    (18, "bootstrap".to_owned()),
                    (19, "bootstrap".to_owned()),
                    (20, "bootstrap".to_owned()),
                    (21, "bootstrap".to_owned()),
                    (22, "bootstrap".to_owned()),
                    (23, "bootstrap".to_owned()),
                    (24, "bootstrap".to_owned()),
                    (25, "bootstrap".to_owned()),
                    (26, "bootstrap".to_owned()),
                    (27, "bootstrap".to_owned()),
                    (28, "bootstrap".to_owned()),
                    (29, "bootstrap".to_owned()),
                    (30, "bootstrap".to_owned()),
                    (31, "bootstrap".to_owned()),
                    (32, "bootstrap".to_owned()),
                    (33, "bootstrap".to_owned())
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

    async fn search_text_objects(pool: &sqlx::PgPool) -> (bool, bool, i64, bool) {
        sqlx::query_as(
            "SELECT EXISTS (SELECT 1 FROM pg_attribute \
                     WHERE attrelid = 'public.stream_events'::regclass \
                       AND attname = 'search_text' AND NOT attisdropped), \
                    EXISTS (SELECT 1 FROM pg_trigger \
                     WHERE tgname = 'stream_events_search_text' AND NOT tgisinternal), \
                    (SELECT count(*) FROM pg_proc p \
                     JOIN pg_namespace n ON n.oid = p.pronamespace \
                     WHERE n.nspname = 'public' AND p.proname \
                       IN ('stream_events_search_text', 'stream_events_search_text_of')), \
                    to_regclass('public.idx_stream_events_session_search_trgm') IS NOT NULL",
        )
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn insert_and_read_plain_event(pool: &sqlx::PgPool) -> String {
        sqlx::raw_sql(
            "INSERT INTO sessions (id, machine_id, working_dir) VALUES ('plain', 'm', '/w') \
             ON CONFLICT DO NOTHING",
        )
        .execute(pool)
        .await
        .unwrap();
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO stream_events (session_id, event_type, payload) \
             VALUES ('plain', 'message', '{\"text\": \"t\"}') RETURNING id",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        sqlx::query_scalar("SELECT payload->>'text' FROM stream_events WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[test]
    fn the_search_baseline_drops_only_the_search_text_index() {
        let (full, baseline) = (super::SEARCH[0].sql, super::SEARCH_BASELINE.as_str());
        assert!(full.contains("search_text"));
        assert!(!baseline.contains("search_text"));
        assert!(!baseline.contains("idx_stream_events_session_search_trgm"));
        for needle in ["btree_gin", "pg_trgm", "idx_search_chunks_session_body_trgm"] {
            assert!(baseline.contains(needle), "{needle}");
        }
        assert_eq!(full.lines().count() - baseline.lines().count(), 3);
        assert_eq!(
            hex::encode(Sha256::digest(baseline.as_bytes())),
            "d6148ad378e097f37304983f9ddcf68533c766c5bf0d1eadee005e0a615a59a0",
            "a database first searched after the column drop records this checksum"
        );
    }

    #[tokio::test]
    async fn a_fresh_install_ends_without_the_search_text_column() {
        with_isolated_database("search_text_dropped", |pool| async move {
            provisioned(&pool).await;
            assert_eq!(search_text_objects(&pool).await, (false, false, 0, false));
            assert_eq!(insert_and_read_plain_event(&pool).await, "t");
            let recorded: Vec<u8> = sqlx::query_scalar(
                "SELECT checksum FROM _cctui_schema_migrations \
                 WHERE package = $1 AND version = 1",
            )
            .bind(super::SEARCH_PACKAGE)
            .fetch_one(&pool)
            .await
            .unwrap();
            let variant = Sha256::digest(super::SEARCH_BASELINE.as_bytes()).to_vec();
            assert_eq!(recorded, variant);
        })
        .await;
    }

    #[tokio::test]
    async fn a_searchless_install_ends_without_the_search_text_column() {
        with_isolated_database("search_text_dropped_searchless", |pool| async move {
            cctui_server_kernel::schema::core::bootstrap(&pool).await.unwrap();
            let mut conn = cctui_server_kernel::schema::migration_connection(&pool).await.unwrap();
            super::provision(&mut conn, false).await.unwrap();
            drop(conn);
            let versions: Vec<i64> =
                installations(&pool, "workspace").await.into_iter().map(|(v, _)| v).collect();
            assert_eq!(versions.last(), Some(&33));
            assert!(installations(&pool, super::SEARCH_PACKAGE).await.is_empty());
            assert_eq!(search_text_objects(&pool).await, (false, false, 0, false));
            assert_eq!(insert_and_read_plain_event(&pool).await, "t");

            let mut conn = cctui_server_kernel::schema::migration_connection(&pool).await.unwrap();
            super::provision(&mut conn, true).await.unwrap();
            drop(conn);
            assert_eq!(installations(&pool, super::SEARCH_PACKAGE).await.len(), 2);
            assert_eq!(search_text_objects(&pool).await, (false, false, 0, false));
        })
        .await;
    }

    #[tokio::test]
    async fn a_database_searched_before_the_column_drop_keeps_its_search_ledger() {
        with_isolated_database("search_text_drop_upgrade", |pool| async move {
            use cctui_server_kernel::schema::{adopt, install_package};
            cctui_server_kernel::schema::core::bootstrap(&pool).await.unwrap();
            let mut conn = cctui_server_kernel::schema::migration_connection(&pool).await.unwrap();
            let before_drop = &super::MIGRATIONS[..30];
            install_package(&mut conn, "workspace", &before_drop[..1], false).await.unwrap();
            adopt(&mut conn, "workspace", before_drop[1]).await.unwrap();
            install_package(&mut conn, "workspace", before_drop, false).await.unwrap();
            install_package(&mut conn, super::SEARCH_PACKAGE, super::SEARCH, false).await.unwrap();
            let searched = installations(&pool, super::SEARCH_PACKAGE).await;
            super::provision(&mut conn, true).await.unwrap();
            drop(conn);
            assert_eq!(installations(&pool, super::SEARCH_PACKAGE).await, searched);
            assert_eq!(search_text_objects(&pool).await, (false, false, 0, false));
            assert_eq!(insert_and_read_plain_event(&pool).await, "t");
        })
        .await;
    }

    const PREVIOUS_INSERT_ONE: &str = "INSERT INTO stream_events \
     (session_id, event_type, payload, turn_id) \
     SELECT $1, $2, $3, $4 WHERE EXISTS ( \
         SELECT 1 FROM sessions WHERE id = $1 AND machine_uuid = $5 AND user_id = $6) \
     ON CONFLICT DO NOTHING \
     RETURNING id";

    const PREVIOUS_INSERT_MANY: &str = "WITH src AS ( \
         SELECT nextval(pg_get_serial_sequence('stream_events', 'id')) AS id, r.* \
         FROM ( \
             SELECT u.* \
             FROM unnest($1::text[], $2::text[], $3::jsonb[], $4::uuid[]) \
                  WITH ORDINALITY AS u(session_id, event_type, payload, turn_id, ord) \
             WHERE EXISTS ( \
                 SELECT 1 FROM sessions s \
                 WHERE s.id = u.session_id AND s.machine_uuid = $5 AND s.user_id = $6) \
             ORDER BY u.ord \
         ) r \
     ), ins AS ( \
         INSERT INTO stream_events (id, session_id, event_type, payload, turn_id) \
         SELECT id, session_id, event_type, payload, turn_id \
         FROM src ORDER BY ord \
         ON CONFLICT DO NOTHING \
         RETURNING id \
     ) \
     SELECT src.ord, ins.id FROM ins JOIN src USING (id)";

    const PREVIOUS_SESSION_ENDED: &str = "INSERT INTO stream_events (session_id, event_type, payload) \
     SELECT $1, 'session_ended', $2 WHERE EXISTS ( \
         SELECT 1 FROM sessions WHERE id = $1 AND machine_uuid = $3 AND user_id = $4) \
     ON CONFLICT (session_id, event_type, content_hash, \
                  COALESCE(turn_id, '00000000-0000-0000-0000-000000000000'::uuid)) \
     DO NOTHING";

    async fn seed_owned_session(pool: &sqlx::PgPool, id: &str) -> (uuid::Uuid, uuid::Uuid) {
        let (user, machine) = (uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $2)")
            .bind(user)
            .bind(format!("u-{user}"))
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, 'm', $3)")
            .bind(machine)
            .bind(user)
            .bind(format!("mk-{machine}"))
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir) \
             VALUES ($1, 'm', $2, $3, '/w')",
        )
        .bind(id)
        .bind(machine)
        .bind(user)
        .execute(pool)
        .await
        .unwrap();
        (user, machine)
    }

    #[tokio::test]
    async fn the_previous_release_ingest_inserts_still_succeed_after_the_column_drop() {
        with_isolated_database("search_text_drop_mixed_version", |pool| async move {
            provisioned(&pool).await;
            let (user, machine) = seed_owned_session(&pool, "old-pod").await;

            let one: Option<i64> = sqlx::query_scalar(PREVIOUS_INSERT_ONE)
                .bind("old-pod")
                .bind("message")
                .bind(serde_json::json!({ "role": "user", "text": "single" }))
                .bind(None::<uuid::Uuid>)
                .bind(machine)
                .bind(user)
                .fetch_optional(&pool)
                .await
                .unwrap();
            assert!(one.is_some(), "the single-row insert");

            let text = |t: &str| serde_json::json!({ "text": t });
            let many: Vec<(i64, i64)> = sqlx::query_as(PREVIOUS_INSERT_MANY)
                .bind(vec!["old-pod", "old-pod"])
                .bind(vec!["message", "tool_use"])
                .bind(vec![text("first"), text("second")])
                .bind(vec![None::<uuid::Uuid>, None])
                .bind(machine)
                .bind(user)
                .fetch_all(&pool)
                .await
                .unwrap();
            assert_eq!(many.len(), 2, "the batched insert");

            let ended = sqlx::query(PREVIOUS_SESSION_ENDED)
                .bind("old-pod")
                .bind(serde_json::json!({ "reason": "killed" }))
                .bind(machine)
                .bind(user)
                .execute(&pool)
                .await
                .unwrap();
            assert_eq!(ended.rows_affected(), 1, "the session-ended row");

            let stored: Vec<String> = sqlx::query_scalar(
                "SELECT event_type FROM stream_events WHERE session_id = 'old-pod' ORDER BY id",
            )
            .fetch_all(&pool)
            .await
            .unwrap();
            assert_eq!(stored, ["message", "message", "tool_use", "session_ended"]);
        })
        .await;
    }

    /// Rows as 0025 left them: machine + cron + account, machine + pool, dispatcher.
    async fn seed_legacy_presets(
        pool: &sqlx::PgPool,
    ) -> (cctui_server_sessions::presets::test_support::Owner, uuid::Uuid, uuid::Uuid) {
        let owner = cctui_server_sessions::presets::test_support::seed_owner(pool, "tpl-mig").await;
        let account: uuid::Uuid = sqlx::query_scalar(
            "INSERT INTO accounts (user_id, name) VALUES ($1, 'personal') RETURNING id",
        )
        .bind(owner.user)
        .fetch_one(pool)
        .await
        .unwrap();
        let bots: uuid::Uuid = sqlx::query_scalar(
            "INSERT INTO account_pools (user_id, name) VALUES ($1, 'bots') RETURNING id",
        )
        .bind(owner.user)
        .fetch_one(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO presets (user_id, name, prompt, machine_id, working_dir, adapter_id, \
               model, permission_mode, cron, timezone, overlap, enabled, account_id, \
               external_sources, env_keys, sealed_env) \
             VALUES ($1, 'nightly', 'triage', $2, '/w', 'claude-code', 'opus', 'auto', \
               '0 7 * * 1-5', 'Asia/Tokyo', 'skip', false, $3, true, '{TOKEN}', 'v1:00')",
        )
        .bind(owner.user)
        .bind(owner.machine)
        .bind(account)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO presets (user_id, name, prompt, machine_id, working_dir, adapter_id, \
               pool_id, fresh_secs, on_stale, max_batch) \
             VALUES ($1, 'pooled', 'p', $2, '~', 'codex', $3, 600, 'new', 5)",
        )
        .bind(owner.user)
        .bind(owner.machine)
        .bind(bots)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO presets (user_id, name, prompt, dispatcher_id, profile, notify_url, \
               cron) VALUES ($1, 'worker', 'review', $2, 'tonbo', 'https://n8n.example/h', \
               '0 * * * *')",
        )
        .bind(owner.user)
        .bind(owner.dispatcher)
        .execute(pool)
        .await
        .unwrap();
        (owner, account, bots)
    }

    #[tokio::test]
    async fn the_preset_template_migration_keeps_every_existing_preset_field() {
        with_isolated_database("preset_template_migration", |pool| async move {
            provisioned(&pool).await;
            sqlx::raw_sql("ALTER TABLE presets DROP COLUMN template, DROP COLUMN sealed_prompt")
                .execute(&pool)
                .await
                .unwrap();
            let (owner, account, bots) = seed_legacy_presets(&pool).await;
            let rows = "SELECT jsonb_agg(to_jsonb(p) - 'template' - 'sealed_prompt' \
                        ORDER BY name)::text FROM presets p";
            let before: String = sqlx::query_scalar(rows).fetch_one(&pool).await.unwrap();

            let migration = super::MIGRATIONS.iter().find(|m| m.version == 26).unwrap();
            sqlx::raw_sql(migration.sql).execute(&pool).await.unwrap();

            let after: String = sqlx::query_scalar(rows).fetch_one(&pool).await.unwrap();
            assert_eq!(after, before, "every existing column is kept as it was");
            let added: Vec<(serde_json::Value, Option<String>)> =
                sqlx::query_as("SELECT template, sealed_prompt FROM presets ORDER BY name")
                    .fetch_all(&pool)
                    .await
                    .unwrap();
            assert_eq!(added.len(), 3);
            assert!(added.iter().all(|(t, s)| *t == serde_json::json!({}) && s.is_none()));

            let listed =
                cctui_server_sessions::presets::store::list(&pool, Some(owner.user)).await.unwrap();
            let by = |name: &str| listed.iter().find(|p| p.name == name).unwrap();
            let nightly = by("nightly");
            assert_eq!(
                (nightly.cron.as_deref(), nightly.timezone.as_str(), nightly.account_id),
                (Some("0 7 * * 1-5"), "Asia/Tokyo", Some(account))
            );
            assert_eq!(nightly.model.as_deref(), Some("opus"));
            assert_eq!(nightly.env_keys, ["TOKEN"]);
            assert!(!nightly.enabled && nightly.external_sources);
            assert_eq!(nightly.template, cctui_proto::api::presets::SpawnTemplate::default());
            let pooled = by("pooled");
            assert_eq!((pooled.pool_id, pooled.fresh_secs, pooled.max_batch), (Some(bots), 600, 5));
            let worker = by("worker");
            assert_eq!(worker.profile.as_deref(), Some("tonbo"));
            assert_eq!(worker.notify_url.as_deref(), Some("https://n8n.example/h"));

            let id = nightly.id;
            let loaded =
                cctui_server_sessions::inbox::preset::load(&pool, id).await.unwrap().unwrap();
            let req = cctui_server_sessions::inbox::preset::spawn_request(
                &loaded,
                &cctui_server_sessions::inbox::preset::Plan::Spawn,
                "go",
            )
            .unwrap();
            assert_eq!((req.model.as_deref(), req.effort.as_deref()), (Some("opus"), None));
            assert!(req.label_ids.is_empty() && !req.auto_archive);
        })
        .await;
    }

    #[tokio::test]
    async fn the_context_reach_migration_drops_other_users_machines_from_non_admin_items() {
        with_isolated_database("context_reach_migration", |pool| async move {
            provisioned(&pool).await;
            let (a, admin, b) = (uuid::Uuid::new_v4(), uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
            let (ma, mb) = (uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
            for uid in [a, admin, b] {
                sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $1::text, $1::text)")
                    .bind(uid)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            sqlx::query("INSERT INTO user_acls (user_id, scope) VALUES ($1, 'admin')")
                .bind(admin)
                .execute(&pool)
                .await
                .unwrap();
            for (mid, uid) in [(ma, a), (mb, b)] {
                sqlx::query(
                    "INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, $1::text, $1::text)",
                )
                .bind(mid)
                .bind(uid)
                .execute(&pool)
                .await
                .unwrap();
            }
            let (ma, mb) = (ma.to_string(), mb.to_string());
            let ghost = uuid::Uuid::new_v4().to_string();
            let seed = [
                (a, "both", serde_json::json!({"machines": [ma, mb]}), "user", None),
                (a, "only-b", serde_json::json!({"machines": [mb]}), "machine", Some(mb.clone())),
                (a, "b-and-path", serde_json::json!({"machines": [mb], "paths": ["/w"]}), "user", None),
                (admin, "admins", serde_json::json!({"machines": [mb]}), "machine", Some(mb.clone())),
                (a, "own", serde_json::json!({"machines": [ma]}), "machine", Some(ma.clone())),
                (a, "ghost", serde_json::json!({"machines": [ghost]}), "machine", Some(ghost.clone())),
                (a, "open", serde_json::json!({}), "user", None),
            ];
            for (uid, name, filters, scope, scope_ref) in seed {
                sqlx::query(
                    "INSERT INTO context_items (user_id, kind, name, title, filters, scope, scope_ref) \
                     VALUES ($1, 'rule', $2, $2, $3, $4, $5)",
                )
                .bind(uid)
                .bind(name)
                .bind(filters)
                .bind(scope)
                .bind(scope_ref)
                .execute(&pool)
                .await
                .unwrap();
            }
            let rows = "SELECT jsonb_agg(jsonb_build_object('name', name, 'filters', filters, \
                        'scope', scope, 'ref', scope_ref, 'enabled', enabled) ORDER BY name) \
                        FROM context_items";
            let migration = super::MIGRATIONS.iter().find(|m| m.version == 32).unwrap();
            sqlx::raw_sql(migration.sql).execute(&pool).await.unwrap();
            let once: serde_json::Value = sqlx::query_scalar(rows).fetch_one(&pool).await.unwrap();
            sqlx::raw_sql(migration.sql).execute(&pool).await.unwrap();
            let twice: serde_json::Value = sqlx::query_scalar(rows).fetch_one(&pool).await.unwrap();
            assert_eq!(once, twice, "idempotent");

            let by = |name: &str| {
                once.as_array().unwrap().iter().find(|r| r["name"] == name).unwrap().clone()
            };
            let both = by("both");
            assert_eq!(both["filters"]["machines"], serde_json::json!([ma]));
            assert_eq!((&both["scope"], &both["ref"]), (&serde_json::json!("machine"), &serde_json::json!(ma)));
            assert_eq!(both["enabled"], true);
            let only_b = by("only-b");
            assert_eq!(only_b["filters"]["machines"], serde_json::json!([]));
            assert_eq!((&only_b["scope"], &only_b["ref"]), (&serde_json::json!("user"), &serde_json::Value::Null));
            assert_eq!(only_b["enabled"], false, "never widened to every machine of its owner");
            let with_path = by("b-and-path");
            assert_eq!((&with_path["scope"], &with_path["ref"]), (&serde_json::json!("path"), &serde_json::json!("/w")));
            assert_eq!(with_path["enabled"], false);
            assert_eq!(by("admins")["filters"]["machines"], serde_json::json!([mb]), "the admin decides");
            assert_eq!(by("own")["filters"]["machines"], serde_json::json!([ma]));
            assert_eq!(by("ghost")["filters"]["machines"], serde_json::json!([ghost]));
            assert_eq!(by("open")["filters"], serde_json::json!({}));
        })
        .await;
    }

    #[tokio::test]
    async fn the_thread_root_migration_backfills_every_followup_line() {
        with_isolated_database("session_thread_root_migration", |pool| async move {
            provisioned(&pool).await;
            sqlx::raw_sql("ALTER TABLE sessions DROP COLUMN thread_root_id")
                .execute(&pool)
                .await
                .unwrap();
            sqlx::raw_sql(
                "INSERT INTO sessions (id, machine_id, working_dir) VALUES ('a', 'm', '/w'); \
                 INSERT INTO sessions (id, machine_id, working_dir, parent_id, metadata) VALUES \
                   ('b', 'm', '/w', 'a', '{\"relation\": \"followup\"}'), \
                   ('b2', 'm', '/w', 'a', '{\"relation\": \"followup\"}'); \
                 INSERT INTO sessions (id, machine_id, working_dir, parent_id, metadata) VALUES \
                   ('c', 'm', '/w', 'b', '{\"relation\": \"followup\"}'), \
                   ('sub', 'm', '/w', 'b', '{\"relation\": \"subagent\"}'); \
                 INSERT INTO sessions (id, machine_id, working_dir, parent_id, metadata) VALUES \
                   ('fork', 'm', '/w', 'c', '{\"relation\": \"fork\"}'); \
                 INSERT INTO sessions (id, machine_id, working_dir) VALUES ('lone', 'm', '/w')",
            )
            .execute(&pool)
            .await
            .unwrap();

            let migration = super::MIGRATIONS.iter().find(|m| m.version == 27).unwrap();
            sqlx::raw_sql(migration.sql).execute(&pool).await.unwrap();

            let roots: Vec<(String, Option<String>)> =
                sqlx::query_as("SELECT id, thread_root_id FROM sessions ORDER BY id")
                    .fetch_all(&pool)
                    .await
                    .unwrap();
            let root = |id: &str| roots.iter().find(|(s, _)| s == id).unwrap().1.clone();
            for id in ["a", "b", "b2", "c"] {
                assert_eq!(root(id).as_deref(), Some("a"), "{id} is on the line rooted at a");
            }
            for id in ["sub", "fork", "lone"] {
                assert_eq!(root(id), None, "{id} is not a follow-up line");
            }
            let indexed: bool = sqlx::query_scalar(
                "SELECT to_regclass('public.idx_sessions_thread_root_id') IS NOT NULL",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert!(indexed);
        })
        .await;
    }

    #[tokio::test]
    async fn a_session_holds_one_pending_followup_op_and_one_op_per_key() {
        with_isolated_database("session_followup_ops_migration", |pool| async move {
            provisioned(&pool).await;
            sqlx::raw_sql(
                "INSERT INTO sessions (id, machine_id, working_dir) VALUES ('a', 'm', '/w')",
            )
            .execute(&pool)
            .await
            .unwrap();
            let insert = |key: &'static str, state: &'static str| {
                let pool = pool.clone();
                async move {
                    sqlx::query(
                        "INSERT INTO session_followup_ops \
                             (source_session_id, idempotency_key, reason, command_id, state) \
                         VALUES ('a', $1, 'cold', gen_random_uuid(), $2)",
                    )
                    .bind(key)
                    .bind(state)
                    .execute(&pool)
                    .await
                }
            };
            insert("k1", "pending").await.unwrap();
            assert!(insert("k2", "pending").await.is_err(), "a second pending op is refused");
            assert!(insert("k1", "failed").await.is_err(), "a key names one op");
            insert("k2", "failed").await.unwrap();
            insert("k3", "registered").await.unwrap();
            assert!(insert("k4", "done").await.is_err(), "unknown state");
            sqlx::raw_sql("DELETE FROM sessions WHERE id = 'a'").execute(&pool).await.unwrap();
            let left: i64 = sqlx::query_scalar("SELECT count(*) FROM session_followup_ops")
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(left, 0, "ops go with their session");
        })
        .await;
    }

    #[tokio::test]
    async fn a_profile_holds_an_auto_followup_override_and_a_spawn_its_intent() {
        with_isolated_database("profile_auto_followup_migration", |pool| async move {
            provisioned(&pool).await;
            let user = uuid::Uuid::new_v4();
            sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $2)")
                .bind(user)
                .bind(format!("u-{user}"))
                .execute(&pool)
                .await
                .unwrap();
            let unset: Option<serde_json::Value> = sqlx::query_scalar(
                "INSERT INTO session_profiles (user_id, name, harness) \
                 VALUES ($1, 'plain', 'claude-code') RETURNING auto_followup",
            )
            .bind(user)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(unset, None, "an existing profile inherits");
            let set: Option<serde_json::Value> = sqlx::query_scalar(
                "INSERT INTO session_profiles (user_id, name, harness, auto_followup) \
                 VALUES ($1, 'early', 'codex', '{\"enabled\": true}') RETURNING auto_followup",
            )
            .bind(user)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(set, Some(serde_json::json!({"enabled": true})));
            sqlx::raw_sql(
                "INSERT INTO session_auto_followup_intents (spawn_key, auto_followup) \
                 VALUES ('k', '{}')",
            )
            .execute(&pool)
            .await
            .unwrap();
            let again = sqlx::raw_sql(
                "INSERT INTO session_auto_followup_intents (spawn_key, auto_followup) \
                 VALUES ('k', '{}')",
            )
            .execute(&pool)
            .await;
            assert!(again.is_err(), "one intent per spawn key");
        })
        .await;
    }

    #[tokio::test]
    async fn storage_lifecycle_starts_with_no_rule_and_refuses_a_trim_without_a_size() {
        with_isolated_database("storage_lifecycle_rules", |pool| async move {
            provisioned(&pool).await;
            let rules: i64 = sqlx::query_scalar("SELECT count(*) FROM storage_lifecycle_rules")
                .fetch_one(&pool)
                .await
                .unwrap();
            assert_eq!(rules, 0, "the default keeps everything");
            for bad in [
                "INSERT INTO storage_lifecycle_rules (action, after_days) VALUES ('trim', 7)",
                "INSERT INTO storage_lifecycle_rules (action, after_days, above_bytes) \
                 VALUES ('drop', 7, 4096)",
                "INSERT INTO storage_lifecycle_rules (action, after_days, above_bytes) \
                 VALUES ('trim', 0, 4096)",
                "INSERT INTO storage_lifecycle_rules (action, after_days) VALUES ('purge', 7)",
            ] {
                assert!(sqlx::raw_sql(bad).execute(&pool).await.is_err(), "{bad}");
            }
            sqlx::raw_sql(
                "INSERT INTO storage_lifecycle_rules (action, after_days, above_bytes) \
                 VALUES ('trim', 7, 4096); \
                 INSERT INTO storage_lifecycle_rules (action, after_days) VALUES ('drop', 90)",
            )
            .execute(&pool)
            .await
            .unwrap();
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
            assert_eq!(core, [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]);
            assert_eq!(
                installations(&pool, "workspace").await,
                [
                    (1, "bootstrap".to_owned()),
                    (2, "bootstrap".to_owned()),
                    (3, "bootstrap".to_owned()),
                    (4, "bootstrap".to_owned()),
                    (5, "bootstrap".to_owned()),
                    (6, "bootstrap".to_owned()),
                    (7, "bootstrap".to_owned()),
                    (8, "bootstrap".to_owned()),
                    (9, "bootstrap".to_owned()),
                    (10, "bootstrap".to_owned()),
                    (11, "bootstrap".to_owned()),
                    (12, "bootstrap".to_owned()),
                    (13, "bootstrap".to_owned()),
                    (14, "bootstrap".to_owned()),
                    (15, "bootstrap".to_owned()),
                    (16, "bootstrap".to_owned()),
                    (17, "bootstrap".to_owned()),
                    (18, "bootstrap".to_owned()),
                    (19, "bootstrap".to_owned()),
                    (20, "bootstrap".to_owned()),
                    (21, "bootstrap".to_owned()),
                    (22, "bootstrap".to_owned()),
                    (23, "bootstrap".to_owned()),
                    (24, "bootstrap".to_owned()),
                    (25, "bootstrap".to_owned()),
                    (26, "bootstrap".to_owned()),
                    (27, "bootstrap".to_owned()),
                    (28, "bootstrap".to_owned()),
                    (29, "bootstrap".to_owned()),
                    (30, "bootstrap".to_owned()),
                    (31, "bootstrap".to_owned()),
                    (32, "bootstrap".to_owned()),
                    (33, "bootstrap".to_owned())
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
