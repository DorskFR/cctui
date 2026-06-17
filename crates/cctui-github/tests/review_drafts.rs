//! GH-VIEW-4: the review-draft store — open/list/update/delete + inline draft
//! comments, the "one open draft per user+pull" invariant, and connector-scoped
//! isolation.
//!
//! DB-gated like `upsert_idempotency`: point `TEST_DATABASE_URL` at a throwaway
//! Postgres and run with `--ignored`.
//!
//! ```text
//! TEST_DATABASE_URL=postgres://localhost/cctui_github_test \
//!     cargo test -p cctui-github --test review_drafts -- --ignored
//! ```

use cctui_proto::github::{
    CreateDraftComment, DiffSide, DraftAuthorKind, DraftStatus, ReviewVerdict,
};
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
async fn open_is_idempotent_one_draft_per_user_pull() {
    let Some((pool, conn, user)) = setup().await else { return };

    let d1 = cctui_github::open_user_draft(&pool, conn, "o/r", 7, user, ReviewVerdict::Comment)
        .await
        .unwrap();
    let d2 = cctui_github::open_user_draft(&pool, conn, "o/r", 7, user, ReviewVerdict::Approve)
        .await
        .unwrap();
    // Re-opening reuses the same open draft (one open per user+pull).
    assert_eq!(d1.id, d2.id);
    assert_eq!(d1.author_kind, DraftAuthorKind::User);
    assert_eq!(d1.status, DraftStatus::Draft);

    let cnt: i64 =
        pool.fetch_one("SELECT count(*) FROM github.review_drafts").await.unwrap().get(0);
    assert_eq!(cnt, 1);
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL"]
async fn comment_crud_round_trip_and_cascade() {
    let Some((pool, conn, user)) = setup().await else { return };
    let draft = cctui_github::open_user_draft(&pool, conn, "o/r", 7, user, ReviewVerdict::Comment)
        .await
        .unwrap();

    // Add two inline comments instantly — no GitHub round-trip.
    let d = cctui_github::add_comment(&pool, conn, "o/r", 7, draft.id, &comment("a.rs", 12))
        .await
        .unwrap();
    let d = cctui_github::add_comment(&pool, conn, "o/r", 7, draft.id, &comment("b.rs", 3))
        .await
        .unwrap_or(d);
    assert_eq!(d.comments.len(), 2);
    let first = d.comments[0].clone();
    assert_eq!(first.path, "a.rs");
    assert_eq!(first.line, 12);
    assert_eq!(first.side, DiffSide::New);
    assert!(first.github_comment_id.is_none());

    // Edit a comment body in place; the anchor is unchanged.
    let d =
        cctui_github::update_comment(&pool, conn, "o/r", 7, draft.id, first.id, "fixed wording")
            .await
            .unwrap();
    let edited = d.comments.iter().find(|c| c.id == first.id).unwrap();
    assert_eq!(edited.body, "fixed wording");
    assert_eq!(edited.line, 12);

    // Change the verdict.
    let d = cctui_github::update_verdict(
        &pool,
        conn,
        "o/r",
        7,
        draft.id,
        ReviewVerdict::RequestChanges,
    )
    .await
    .unwrap();
    assert_eq!(d.verdict, ReviewVerdict::RequestChanges);

    // Delete one comment.
    let d = cctui_github::delete_comment(&pool, conn, "o/r", 7, draft.id, first.id).await.unwrap();
    assert_eq!(d.comments.len(), 1);

    // Deleting the draft cascades to its remaining comments.
    cctui_github::delete_draft(&pool, conn, "o/r", 7, draft.id).await.unwrap();
    let cc: i64 =
        pool.fetch_one("SELECT count(*) FROM github.review_draft_comments").await.unwrap().get(0);
    assert_eq!(cc, 0);
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL"]
async fn missing_draft_is_not_found() {
    let Some((pool, conn, _user)) = setup().await else { return };
    let err = cctui_github::add_comment(&pool, conn, "o/r", 7, Uuid::new_v4(), &comment("a.rs", 1))
        .await
        .unwrap_err();
    assert_eq!(err, cctui_github::DraftError::NotFound);
    let err = cctui_github::delete_draft(&pool, conn, "o/r", 7, Uuid::new_v4()).await.unwrap_err();
    assert_eq!(err, cctui_github::DraftError::NotFound);
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL"]
async fn drafts_are_scoped_to_their_pull() {
    let Some((pool, conn, user)) = setup().await else { return };
    cctui_github::open_user_draft(&pool, conn, "o/r", 7, user, ReviewVerdict::Comment)
        .await
        .unwrap();
    // A different PR number gets its own (separate) open draft.
    cctui_github::open_user_draft(&pool, conn, "o/r", 8, user, ReviewVerdict::Comment)
        .await
        .unwrap();
    let for_7 = cctui_github::list_drafts(&pool, conn, "o/r", 7, Some(user)).await.unwrap();
    let for_8 = cctui_github::list_drafts(&pool, conn, "o/r", 8, Some(user)).await.unwrap();
    assert_eq!(for_7.len(), 1);
    assert_eq!(for_8.len(), 1);
    assert_ne!(for_7[0].id, for_8[0].id);
}
