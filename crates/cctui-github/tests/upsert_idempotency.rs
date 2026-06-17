//! GH-CONN-3: the upsert functions are idempotent — replaying the same event
//! (or a webhook racing a reconcile) converges on one row, updating it in place.
//!
//! DB-gated like `schema_isolation`: point `TEST_DATABASE_URL` at a throwaway
//! Postgres and run with `--ignored`.
//!
//! ```text
//! TEST_DATABASE_URL=postgres://localhost/cctui_github_test \
//!     cargo test -p cctui-github --test upsert_idempotency -- --ignored
//! ```

use cctui_proto::github::{
    CheckUpsert, PullUpsert, ReviewCommentUpsert, ReviewThreadUpsert, ReviewUpsert,
};
use sqlx::{Executor, Row, postgres::PgPoolOptions};
use uuid::Uuid;

/// Fresh schema + a connector to scope rows to; returns (pool, connector_id).
async fn setup() -> Option<(sqlx::PgPool, Uuid)> {
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
            sqlx::query("INSERT INTO github.connectors (user_id) VALUES ($1) RETURNING id")
                .bind(user_id),
        )
        .await
        .unwrap()
        .get(0);
    Some((pool, connector_id))
}

async fn count(pool: &sqlx::PgPool, table: &str) -> i64 {
    pool.fetch_one(format!("SELECT count(*) FROM github.{table}").as_str())
        .await
        .unwrap()
        .get(0)
}

fn pull() -> PullUpsert {
    PullUpsert {
        node_id: "PR_n".into(),
        repo: "o/r".into(),
        number: 42,
        title: "first".into(),
        state: "open".into(),
        merged: false,
        draft: false,
        mergeable_state: None,
        author: "me".into(),
        head_sha: "sha1".into(),
        base_ref: "main".into(),
        head_ref: "feat".into(),
        gh_created_at: "2026-06-17T00:00:00Z".into(),
        gh_updated_at: "2026-06-17T00:00:00Z".into(),
    }
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL"]
async fn upserts_are_idempotent_and_update_in_place() {
    let Some((pool, cid)) = setup().await else { return };

    // Pull: same key twice = one row, second wins on mutable fields, stable id.
    let mut p = pull();
    let id1 = cctui_github::upsert_pull(&pool, cid, &p).await.unwrap();
    p.title = "second".into();
    p.head_sha = "sha2".into();
    let id2 = cctui_github::upsert_pull(&pool, cid, &p).await.unwrap();
    assert_eq!(id1, id2, "same (connector,repo,number) must reuse the row");
    assert_eq!(count(&pool, "pulls").await, 1);
    let title: String = pool
        .fetch_one(sqlx::query("SELECT title FROM github.pulls WHERE id = $1").bind(id1))
        .await
        .unwrap()
        .get(0);
    assert_eq!(title, "second", "update must overwrite mutable fields");

    // Check: keyed on (connector, repo, head_sha, external_id).
    let mut c = CheckUpsert {
        repo: "o/r".into(),
        head_sha: "sha2".into(),
        external_id: "123".into(),
        name: "ci".into(),
        status: "in_progress".into(),
        conclusion: None,
        details_url: None,
    };
    cctui_github::upsert_check(&pool, cid, &c).await.unwrap();
    c.status = "completed".into();
    c.conclusion = Some("success".into());
    cctui_github::upsert_check(&pool, cid, &c).await.unwrap();
    assert_eq!(count(&pool, "checks").await, 1, "same check key = one row");

    // Review.
    let r = ReviewUpsert {
        repo: "o/r".into(),
        pull_number: 42,
        review_id: 999,
        reviewer: "you".into(),
        state: "approved".into(),
        body: Some("lgtm".into()),
        commit_id: Some("sha2".into()),
        submitted_at: Some("2026-06-17T02:00:00Z".into()),
    };
    cctui_github::upsert_review(&pool, cid, &r).await.unwrap();
    cctui_github::upsert_review(&pool, cid, &r).await.unwrap();
    assert_eq!(count(&pool, "reviews").await, 1);

    // Review thread.
    let t = ReviewThreadUpsert {
        repo: "o/r".into(),
        pull_number: 42,
        thread_node_id: "T_n".into(),
        path: "src/lib.rs".into(),
        side: Some("RIGHT".into()),
        line: Some(10),
        resolved: false,
    };
    cctui_github::upsert_review_thread(&pool, cid, &t).await.unwrap();
    cctui_github::upsert_review_thread(&pool, cid, &t).await.unwrap();
    assert_eq!(count(&pool, "review_threads").await, 1);

    // Review comment.
    let cm = ReviewCommentUpsert {
        repo: "o/r".into(),
        pull_number: 42,
        comment_id: 555,
        thread_node_id: Some("T_n".into()),
        author: "you".into(),
        body: "nit".into(),
        path: Some("src/lib.rs".into()),
        side: Some("RIGHT".into()),
        line: Some(10),
        gh_created_at: "2026-06-17T02:00:00Z".into(),
        gh_updated_at: "2026-06-17T02:00:00Z".into(),
    };
    cctui_github::upsert_review_comment(&pool, cid, &cm).await.unwrap();
    cctui_github::upsert_review_comment(&pool, cid, &cm).await.unwrap();
    assert_eq!(count(&pool, "review_comments").await, 1);
}
