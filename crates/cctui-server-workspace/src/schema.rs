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

mod search;
pub use search::{SEARCH, SEARCH_PACKAGE};

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
                    (29, "bootstrap".to_owned())
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
    async fn search_text_drops_base64_attachments_and_keeps_every_other_row() {
        with_isolated_database("search_text_without_base64", |pool| async move {
            provisioned(&pool).await;
            sqlx::raw_sql(
                "INSERT INTO sessions (id, machine_id, working_dir) VALUES ('s', 'm', '/w')",
            )
            .execute(&pool)
            .await
            .unwrap();
            let insert = |payload: serde_json::Value| {
                let pool = pool.clone();
                async move {
                    sqlx::query_as::<_, (String, String)>(
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
                    .unwrap()
                }
            };
            let data = "iVBORw0KGgoAAAANSUhEUgAA+/".repeat(40);
            let image = serde_json::json!({ "kind": "tool_result", "content": [
                { "type": "text", "text": "Screenshot saved" },
                { "type": "image", "source": {
                    "type": "base64", "media_type": "image/png", "data": data } },
            ]});
            let (stored, _) = insert(image).await;
            assert!(!stored.contains(&data[..200]), "{stored}");
            assert!(stored.contains("Screenshot saved") && stored.contains("\"data\": \"\""));
            let text = format!("see data:image/png;base64,{data}= ok");
            let url = serde_json::json!({ "text": text });
            let (stored, _) = insert(url).await;
            assert!(stored.starts_with("see data:image/png;base64, ok"), "{stored}");

            let plain = [
                serde_json::json!({ "text": "decode the base64 output" }),
                serde_json::json!({ "tool": "Bash", "input": { "command": "base64 -d f" } }),
                serde_json::json!({ "content": format!("base64 {}", "ab".repeat(300)) }),
                serde_json::json!({
                    "text": "base64", "input": { "data": "hello world ".repeat(30) } }),
                serde_json::json!({ "content": [{ "type": "image", "source": {
                    "type": "base64", "media_type": "image/png", "data": "iVBORw0KGgo=" } }] }),
            ];
            for payload in plain {
                let shown = payload.to_string();
                let (stored, former) = insert(payload).await;
                assert_eq!(stored, former, "{shown}");
            }

            let legacy: i64 = sqlx::query_scalar(
                "INSERT INTO stream_events (session_id, event_type, payload, search_text) \
                 VALUES ('s', 'tool_use', $1, $2) RETURNING id",
            )
            .bind(serde_json::json!({ "content": [{ "type": "image", "source": {
                "type": "base64", "media_type": "image/png", "data": format!("{data}x") } }] }))
            .bind(format!("legacy {data}"))
            .fetch_one(&pool)
            .await
            .unwrap();
            let recomputed: String = sqlx::query_scalar(
                "UPDATE stream_events SET payload = payload WHERE id = $1 RETURNING search_text",
            )
            .bind(legacy)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert!(!recomputed.contains(&data[..200]), "{recomputed}");
            assert!(!recomputed.starts_with("legacy"));
        })
        .await;
    }

    /// Rows as 0025 left them: machine + cron + account, machine + pool, dispatcher.
    async fn seed_legacy_presets(
        pool: &sqlx::PgPool,
    ) -> (crate::presets::store_tests::Owner, uuid::Uuid, uuid::Uuid) {
        let owner = crate::presets::store_tests::seed_owner(pool, "tpl-mig").await;
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

            let listed = crate::presets::store::list(&pool, Some(owner.user)).await.unwrap();
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
            let loaded = crate::inbox::preset::load(&pool, id).await.unwrap().unwrap();
            let req = crate::inbox::preset::spawn_request(
                &loaded,
                &crate::inbox::preset::Plan::Spawn,
                "go",
            )
            .unwrap();
            assert_eq!((req.model.as_deref(), req.effort.as_deref()), (Some("opus"), None));
            assert!(req.label_ids.is_empty() && !req.auto_archive);
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
                    (29, "bootstrap".to_owned())
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
