//! GH-AGENT-2: DB-gated tests for the agent review-draft store functions the MCP
//! review tool drives — `open_agent_draft` (author=agent+`session_id`, open-or-
//! reuse) and `set_summary` (set/append summary + verdict). Also covers the
//! anchor write through `add_comment` under an agent draft and the
//! auth→author mapping (the session id becomes `author_session_id`).
//!
//! DB-gated like `review_drafts`: point `TEST_DATABASE_URL` at a throwaway
//! Postgres and run with `--ignored`.
//!
//! ```text
//! TEST_DATABASE_URL=postgres://localhost/cctui_github_test \
//!     cargo test -p cctui-github --test mcp_drafts -- --ignored
//! ```

use cctui_proto::github::{CreateDraftComment, DiffSide, DraftAuthorKind, ReviewVerdict};
use sqlx::postgres::PgPoolOptions;
use sqlx::{Executor, Row};
use uuid::Uuid;

/// Fresh schema + a connector + its owning user; returns (pool, connector).
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
            sqlx::query(
                "INSERT INTO github.connectors (user_id, name, credential_kind, encrypted_credential) \
                 VALUES ($1, 'test', 'pat', 'x') RETURNING id",
            )
            .bind(user_id),
        )
        .await
        .unwrap()
        .get(0);
    Some((pool, connector_id))
}

fn comment(path: &str, line: u32) -> CreateDraftComment {
    CreateDraftComment {
        path: path.into(),
        side: DiffSide::New,
        line,
        start_line: None,
        body: "agent note".into(),
        in_reply_to: None,
    }
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL"]
async fn open_agent_draft_maps_session_to_author_and_reuses() {
    let Some((pool, conn)) = setup().await else { return };
    let session = "sess-abc-123";

    // First open creates an agent draft authored by the session.
    let d1 = cctui_github::open_agent_draft(&pool, conn, "o/n", 7, session, ReviewVerdict::Comment)
        .await
        .unwrap();
    assert_eq!(d1.author_kind, DraftAuthorKind::Agent);
    assert_eq!(d1.author_session_id.as_deref(), Some(session));
    assert!(d1.author_user_id.is_none());

    // Second open reuses the SAME row (no duplicate agent draft per session+pull).
    let d2 = cctui_github::open_agent_draft(&pool, conn, "o/n", 7, session, ReviewVerdict::Approve)
        .await
        .unwrap();
    assert_eq!(d1.id, d2.id);

    let count: i64 = pool
        .fetch_one(
            sqlx::query(
                "SELECT count(*) FROM github.review_drafts \
                 WHERE author_kind = 'agent' AND author_session_id = $1",
            )
            .bind(session),
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(count, 1);

    // A different session gets its own draft on the same PR.
    let other =
        cctui_github::open_agent_draft(&pool, conn, "o/n", 7, "sess-xyz", ReviewVerdict::Comment)
            .await
            .unwrap();
    assert_ne!(other.id, d1.id);
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL"]
async fn agent_draft_accepts_anchored_comments() {
    let Some((pool, conn)) = setup().await else { return };
    let session = "sess-1";
    let d = cctui_github::open_agent_draft(&pool, conn, "o/n", 1, session, ReviewVerdict::Comment)
        .await
        .unwrap();

    let refreshed =
        cctui_github::add_comment(&pool, conn, "o/n", 1, d.id, &comment("src/main.rs", 12))
            .await
            .unwrap();
    assert_eq!(refreshed.comments.len(), 1);
    assert_eq!(refreshed.comments[0].path, "src/main.rs");
    assert_eq!(refreshed.comments[0].line, 12);
    assert_eq!(refreshed.comments[0].side, DiffSide::New);
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL"]
async fn set_summary_sets_then_appends_and_sets_verdict() {
    let Some((pool, conn)) = setup().await else { return };
    let session = "sess-2";
    let d = cctui_github::open_agent_draft(&pool, conn, "o/n", 5, session, ReviewVerdict::Comment)
        .await
        .unwrap();

    // Set (replace) the summary + a verdict.
    let (_, s1) = cctui_github::set_summary(
        &pool,
        conn,
        "o/n",
        5,
        d.id,
        &cctui_github::SummaryUpdate {
            summary: "Overall LGTM.",
            verdict: ReviewVerdict::Approve,
            append: false,
        },
    )
    .await
    .unwrap();
    assert_eq!(s1, "Overall LGTM.");

    // Append adds a blank-line-separated block.
    let (_, s2) = cctui_github::set_summary(
        &pool,
        conn,
        "o/n",
        5,
        d.id,
        &cctui_github::SummaryUpdate {
            summary: "One nit on line 12.",
            verdict: ReviewVerdict::RequestChanges,
            append: true,
        },
    )
    .await
    .unwrap();
    assert_eq!(s2, "Overall LGTM.\n\nOne nit on line 12.");

    // The verdict was updated by the second call.
    let verdict: String = pool
        .fetch_one(sqlx::query("SELECT verdict FROM github.review_drafts WHERE id = $1").bind(d.id))
        .await
        .unwrap()
        .get(0);
    assert_eq!(verdict, "request_changes");
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL"]
async fn set_summary_on_missing_draft_is_not_found() {
    let Some((pool, conn)) = setup().await else { return };
    let res = cctui_github::set_summary(
        &pool,
        conn,
        "o/n",
        9,
        Uuid::new_v4(),
        &cctui_github::SummaryUpdate {
            summary: "x",
            verdict: ReviewVerdict::Comment,
            append: false,
        },
    )
    .await;
    assert_eq!(res.unwrap_err(), cctui_github::DraftError::NotFound);
}
