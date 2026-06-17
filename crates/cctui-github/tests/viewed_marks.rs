//! GH-VIEW-6: the blob-keyed reviewed-mark store — mark idempotency, the
//! per-file blob-SHA re-flag behaviour, unmark, and connector/user isolation.
//!
//! DB-gated like `review_drafts`: point `TEST_DATABASE_URL` at a throwaway
//! Postgres and run with `--ignored`.
//!
//! ```text
//! TEST_DATABASE_URL=postgres://localhost/cctui_github_test \
//!     cargo test -p cctui-github --test viewed_marks -- --ignored
//! ```

use sqlx::postgres::PgPoolOptions;
use sqlx::{Executor, Row};
use uuid::Uuid;

/// Fresh schema + a connector + its owning user; returns (pool, connector, user).
async fn setup() -> Option<(sqlx::PgPool, Uuid, Uuid)> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;
    let pool = PgPoolOptions::new().max_connections(4).connect(&url).await.expect("connect");
    pool.execute("DROP SCHEMA IF EXISTS github CASCADE").await.unwrap();
    pool.execute(
        "CREATE TABLE IF NOT EXISTS public.users (id UUID PRIMARY KEY DEFAULT gen_random_uuid())",
    )
    .await
    .unwrap();
    pool.execute("TRUNCATE public.users CASCADE").await.unwrap();
    cctui_github::migrate(&pool).await.expect("migrate");

    let user_id: Uuid = pool
        .fetch_one("INSERT INTO public.users DEFAULT VALUES RETURNING id")
        .await
        .unwrap()
        .get(0);
    let connector_id: Uuid = pool
        .fetch_one(
            sqlx::query(
                "INSERT INTO github.connectors \
                    (user_id, name, credential_kind, encrypted_credential) \
                 VALUES ($1, 'test', 'pat', 'x') RETURNING id",
            )
            .bind(user_id),
        )
        .await
        .unwrap()
        .get(0);
    Some((pool, connector_id, user_id))
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL"]
async fn mark_is_idempotent_one_row_per_file() {
    let Some((pool, conn, user)) = setup().await else { return };

    // Mark the same path twice with the same blob SHA: one row, no error.
    cctui_github::mark_viewed(&pool, user, conn, "o/r", 7, "src/a.rs", "sha_a").await.unwrap();
    cctui_github::mark_viewed(&pool, user, conn, "o/r", 7, "src/a.rs", "sha_a").await.unwrap();

    let cnt: i64 = pool.fetch_one("SELECT count(*) FROM github.viewed_marks").await.unwrap().get(0);
    assert_eq!(cnt, 1);

    let marks = cctui_github::list_viewed_marks(&pool, user, conn, "o/r", 7).await.unwrap();
    assert_eq!(marks.len(), 1);
    assert_eq!(marks[0].path, "src/a.rs");
    assert_eq!(marks[0].blob_sha, "sha_a");
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL"]
async fn re_marking_after_a_push_updates_blob_sha_in_place() {
    let Some((pool, conn, user)) = setup().await else { return };

    cctui_github::mark_viewed(&pool, user, conn, "o/r", 7, "src/a.rs", "sha_a").await.unwrap();
    // A push changed the file; the reviewer re-marks it against the new content.
    cctui_github::mark_viewed(&pool, user, conn, "o/r", 7, "src/a.rs", "sha_a2").await.unwrap();

    let marks = cctui_github::list_viewed_marks(&pool, user, conn, "o/r", 7).await.unwrap();
    assert_eq!(marks.len(), 1, "still one row per file");
    assert_eq!(marks[0].blob_sha, "sha_a2", "blob SHA updated in place");
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL"]
async fn re_flag_keys_off_stored_blob_sha_per_file() {
    let Some((pool, conn, user)) = setup().await else { return };

    // Two files marked at one head.
    cctui_github::mark_viewed(&pool, user, conn, "o/r", 7, "src/a.rs", "sha_a").await.unwrap();
    cctui_github::mark_viewed(&pool, user, conn, "o/r", 7, "src/b.rs", "sha_b").await.unwrap();

    // After a push: a.rs unchanged (sha_a), b.rs changed (sha_b2). The stored
    // marks are unchanged; the re-flag is a pure comparison against the current
    // diff's blob SHAs — only b.rs re-flags as unreviewed.
    let marks = cctui_github::list_viewed_marks(&pool, user, conn, "o/r", 7).await.unwrap();
    let by_path = |p: &str| marks.iter().find(|m| m.path == p).map(|m| m.blob_sha.clone());

    let current = |p: &str| match p {
        "src/a.rs" => Some("sha_a"),  // unchanged
        "src/b.rs" => Some("sha_b2"), // changed
        _ => None,
    };
    assert!(cctui_github::is_still_reviewed(&by_path("src/a.rs").unwrap(), current("src/a.rs")));
    assert!(!cctui_github::is_still_reviewed(&by_path("src/b.rs").unwrap(), current("src/b.rs")));
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL"]
async fn unmark_removes_only_that_file_and_is_not_found_when_absent() {
    let Some((pool, conn, user)) = setup().await else { return };

    cctui_github::mark_viewed(&pool, user, conn, "o/r", 7, "src/a.rs", "sha_a").await.unwrap();
    cctui_github::mark_viewed(&pool, user, conn, "o/r", 7, "src/b.rs", "sha_b").await.unwrap();

    cctui_github::unmark_viewed(&pool, user, conn, "o/r", 7, "src/a.rs").await.unwrap();
    let marks = cctui_github::list_viewed_marks(&pool, user, conn, "o/r", 7).await.unwrap();
    assert_eq!(marks.len(), 1);
    assert_eq!(marks[0].path, "src/b.rs");

    // Unmarking an unmarked file is NotFound.
    let err = cctui_github::unmark_viewed(&pool, user, conn, "o/r", 7, "src/a.rs").await;
    assert_eq!(err, Err(cctui_github::ViewedError::NotFound));
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL"]
async fn marks_are_scoped_per_user() {
    let Some((pool, conn, user)) = setup().await else { return };
    let other: Uuid = pool
        .fetch_one("INSERT INTO public.users DEFAULT VALUES RETURNING id")
        .await
        .unwrap()
        .get(0);

    cctui_github::mark_viewed(&pool, user, conn, "o/r", 7, "src/a.rs", "sha_a").await.unwrap();

    let mine = cctui_github::list_viewed_marks(&pool, user, conn, "o/r", 7).await.unwrap();
    let theirs = cctui_github::list_viewed_marks(&pool, other, conn, "o/r", 7).await.unwrap();
    assert_eq!(mine.len(), 1);
    assert_eq!(theirs.len(), 0, "another user sees none of my marks");
}
