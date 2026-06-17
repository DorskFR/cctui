//! GH-VIEW-5: DB-gated tests for the publish + pull-down database layer —
//! `mark_published` (status flip + github_comment_id backfill, transactional) and
//! `list_open_threads` (pull-down read, open-only, grouped).
//!
//! DB-gated like `review_drafts` / `upsert_idempotency`: point `TEST_DATABASE_URL`
//! at a throwaway Postgres and run with `--ignored`.
//!
//! ```text
//! TEST_DATABASE_URL=postgres://localhost/cctui_github_test \
//!     cargo test -p cctui-github --test publish -- --ignored
//! ```

use cctui_proto::github::{
    CreateDraftComment, DiffSide, DraftStatus, ReviewCommentUpsert, ReviewThreadUpsert,
    ReviewVerdict,
};
use sqlx::postgres::PgPoolOptions;
use sqlx::Executor;
use tokio::sync::broadcast;
use uuid::Uuid;

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

    let user_id: Uuid = sqlx::query_scalar("INSERT INTO public.users DEFAULT VALUES RETURNING id")
        .fetch_one(&pool)
        .await
        .unwrap();
    let connector_id: Uuid = sqlx::query_scalar(
        "INSERT INTO github.connectors (user_id, name, credential_kind, encrypted_credential) \
         VALUES ($1, 'test', 'pat', 'x') RETURNING id",
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    Some((pool, connector_id, user_id))
}

fn comment(path: &str, line: u32) -> CreateDraftComment {
    CreateDraftComment {
        path: path.into(),
        side: DiffSide::New,
        line,
        start_line: None,
        body: "looks off".into(),
        in_reply_to: None,
    }
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL"]
async fn mark_published_flips_status_and_backfills_github_ids() {
    let Some((pool, conn, user)) = setup().await else { return };

    let d = cctui_github::open_user_draft(&pool, conn, "o/r", 7, user, ReviewVerdict::Comment)
        .await
        .unwrap();
    let d = cctui_github::add_comment(&pool, conn, "o/r", 7, d.id, &comment("a.rs", 1))
        .await
        .unwrap();
    let d = cctui_github::add_comment(&pool, conn, "o/r", 7, d.id, &comment("a.rs", 2))
        .await
        .unwrap();
    assert_eq!(d.status, DraftStatus::Draft);
    let c0 = d.comments[0].id;
    let c1 = d.comments[1].id;

    let published = cctui_github::mark_published(
        &pool,
        conn,
        "o/r",
        7,
        d.id,
        &[(c0, 1001), (c1, 1002)],
    )
    .await
    .unwrap();

    assert_eq!(published.status, DraftStatus::Published);
    let by_id: std::collections::HashMap<_, _> =
        published.comments.iter().map(|c| (c.id, c.github_comment_id)).collect();
    assert_eq!(by_id[&c0], Some(1001));
    assert_eq!(by_id[&c1], Some(1002));

    // Re-publishing an already-published draft is a no-op miss (status guard).
    let again = cctui_github::mark_published(&pool, conn, "o/r", 7, d.id, &[]).await;
    assert!(again.is_err(), "publishing twice must not succeed");
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL"]
async fn list_open_threads_returns_open_with_comments_grouped() {
    let Some((pool, conn, _user)) = setup().await else { return };
    let (tx, _rx) = broadcast::channel(16);

    // One open thread with two comments + one resolved thread (excluded).
    let open = ReviewThreadUpsert {
        repo: "o/r".into(),
        pull_number: 7,
        thread_node_id: "T_open".into(),
        path: "a.rs".into(),
        side: Some("RIGHT".into()),
        line: Some(10),
        resolved: false,
    };
    let resolved = ReviewThreadUpsert { thread_node_id: "T_done".into(), resolved: true, ..open.clone() };
    cctui_github::upsert_review_thread(&pool, &tx, conn, &open).await.unwrap();
    cctui_github::upsert_review_thread(&pool, &tx, conn, &resolved).await.unwrap();

    for (cid, ts) in [(1i64, "2026-06-17T00:00:00Z"), (2, "2026-06-17T01:00:00Z")] {
        let c = ReviewCommentUpsert {
            repo: "o/r".into(),
            pull_number: 7,
            comment_id: cid,
            thread_node_id: Some("T_open".into()),
            author: "octocat".into(),
            body: format!("comment {cid}"),
            path: Some("a.rs".into()),
            side: Some("RIGHT".into()),
            line: Some(10),
            gh_created_at: ts.into(),
            gh_updated_at: ts.into(),
        };
        cctui_github::upsert_review_comment(&pool, &tx, conn, &c).await.unwrap();
    }

    let threads = cctui_github::list_open_threads(&pool, conn, "o/r", 7).await.unwrap();
    assert_eq!(threads.len(), 1, "resolved thread is excluded");
    assert_eq!(threads[0].thread_node_id, "T_open");
    assert_eq!(threads[0].comments.len(), 2);
    // Oldest-first ordering by gh_created_at.
    assert_eq!(threads[0].comments[0].body, "comment 1");
    assert_eq!(threads[0].comments[1].body, "comment 2");
}
