//! GH-PKG-2: the load-bearing `search_path` + `DROP SCHEMA` behaviour.
//!
//! These tests need a throwaway Postgres. Point `TEST_DATABASE_URL` at one and
//! run with `--ignored`:
//!
//! ```text
//! TEST_DATABASE_URL=postgres://localhost/cctui_github_test \
//!     cargo test -p cctui-github --test schema_isolation -- --ignored
//! ```
//!
//! Each test owns the connection: it starts by dropping the `github` schema so
//! reruns are clean.

use sqlx::postgres::PgPoolOptions;
use sqlx::{Executor, Row};

async fn pool() -> Option<sqlx::PgPool> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new().max_connections(4).connect(&url).await.expect("connect");
    // Reset: drop our schema, and stand up a minimal core `users` table so the
    // one-directional FK in migration 001 resolves.
    pool.execute("DROP SCHEMA IF EXISTS github CASCADE").await.unwrap();
    pool.execute(
        "CREATE TABLE IF NOT EXISTS public.users (id UUID PRIMARY KEY DEFAULT gen_random_uuid())",
    )
    .await
    .unwrap();
    // Clear any users left by a prior test so per-test counts are deterministic.
    pool.execute("TRUNCATE public.users CASCADE").await.unwrap();
    Some(pool)
}

/// `migrate()` creates `github._sqlx_migrations` (not `public._sqlx_migrations`),
/// proving the `search_path` pin isolates sqlx's own bookkeeping.
#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL"]
async fn migrations_tracked_in_github_schema() {
    let Some(pool) = pool().await else { return };

    cctui_github::migrate(&pool).await.expect("migrate");

    // The bookkeeping table exists in `github`, ...
    let in_github: bool = pool
        .fetch_one(
            "SELECT EXISTS (SELECT 1 FROM information_schema.tables \
             WHERE table_schema = 'github' AND table_name = '_sqlx_migrations')",
        )
        .await
        .unwrap()
        .get(0);
    assert!(in_github, "_sqlx_migrations must live in github schema");

    // ... and NOT in public (core's history is untouched).
    let in_public: bool = pool
        .fetch_one(
            "SELECT EXISTS (SELECT 1 FROM information_schema.tables \
             WHERE table_schema = 'public' AND table_name = '_sqlx_migrations')",
        )
        .await
        .unwrap()
        .get(0);
    assert!(!in_public, "core public._sqlx_migrations must NOT be created by us");

    // The migration's own table also landed in github.
    let connectors: bool = pool
        .fetch_one(
            "SELECT EXISTS (SELECT 1 FROM information_schema.tables \
             WHERE table_schema = 'github' AND table_name = 'connectors')",
        )
        .await
        .unwrap()
        .get(0);
    assert!(connectors, "github.connectors must exist");

    // migrate() is idempotent.
    cctui_github::migrate(&pool).await.expect("re-migrate idempotent");
}

/// Regression for the prod-breaking `search_path` leak: after `migrate()`, an
/// *unqualified* core query must still resolve against `public`. The original
/// bug returned the search_path-pinned connection to the pool, so a reused
/// connection saw `search_path = github` and failed core queries with
/// "relation \"users\" does not exist". We acquire more connections than the
/// pool holds to force reuse of the migration connection (if it leaked back).
#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL"]
async fn migrate_does_not_leak_search_path_into_pool() {
    let Some(pool) = pool().await else { return };

    cctui_github::migrate(&pool).await.expect("migrate");

    // `public.users` exists (created in `pool()`); an UNQUALIFIED `users` must
    // resolve to it. Loop past max_connections (4) so a leaked, github-pinned
    // connection would be handed back and break this.
    for i in 0..12 {
        let row = pool.fetch_one("SELECT count(*) FROM users").await.unwrap_or_else(|e| {
            panic!("unqualified core query #{i} failed (search_path leak?): {e}")
        });
        let _: i64 = row.get(0);
    }
}

/// `uninstall()` drops everything github while leaving core intact, even with a
/// live `github`->core FK and a referenced core row.
#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL"]
async fn uninstall_drops_github_leaving_core_intact() {
    let Some(pool) = pool().await else { return };
    cctui_github::migrate(&pool).await.expect("migrate");

    // Insert a core user and a github.connector that FKs into it.
    let user_id: uuid::Uuid = pool
        .fetch_one("INSERT INTO public.users DEFAULT VALUES RETURNING id")
        .await
        .unwrap()
        .get(0);
    pool.execute(
        sqlx::query(
            "INSERT INTO github.connectors (user_id, name, credential_kind, encrypted_credential) \
             VALUES ($1, 'test', 'pat', 'x')",
        )
        .bind(user_id),
    )
    .await
    .unwrap();

    cctui_github::uninstall(&pool).await.expect("uninstall");

    // github schema is gone...
    let schema_exists: bool = pool
        .fetch_one(
            "SELECT EXISTS (SELECT 1 FROM information_schema.schemata WHERE schema_name = 'github')",
        )
        .await
        .unwrap()
        .get(0);
    assert!(!schema_exists, "github schema must be dropped");

    // ...core users table + its row survive untouched (no cascade reached it).
    let user_count: i64 = pool.fetch_one("SELECT count(*) FROM public.users").await.unwrap().get(0);
    assert_eq!(user_count, 1, "core users row must survive uninstall");

    // uninstall() is idempotent.
    cctui_github::uninstall(&pool).await.expect("re-uninstall idempotent");

    // And a fresh migrate() works again after teardown.
    cctui_github::migrate(&pool).await.expect("re-migrate after uninstall");
}
