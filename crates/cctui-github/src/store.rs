//! GH-CONN-3: typed, idempotent upserts for synced GitHub PR state.
//!
//! These functions are the **spine** the rest of Epic 1+ builds on: the webhook
//! (GH-CONN-2) and reconcile poll (GH-CONN-4) both parse GitHub objects into the
//! `*Upsert` proto types and call the functions here; the inbox (GH-UI-1), diff
//! viewer (GH-VIEW-*), and classifier tie-in (GH-CLS-1) read the rows back.
//!
//! Every write is an idempotent `INSERT … ON CONFLICT … DO UPDATE` keyed on
//! GitHub's own stable ids (scoped to the connector), so replaying the same
//! event — or a webhook racing a reconcile — converges on one row instead of
//! duplicating. `synced_at` is bumped to the server clock on every touch.
//!
//! No credentials or raw payloads are logged here; callers pass already-parsed,
//! credential-free `*Upsert` values.

use cctui_proto::github::{
    CheckUpsert, GithubEventKind, GithubEventPayload, PullUpsert, ReviewCommentUpsert,
    ReviewThreadUpsert, ReviewUpsert,
};
use cctui_proto::ws::ServerEvent;
use sqlx::PgPool;
use tokio::sync::broadcast;
use uuid::Uuid;

/// Broadcast handle for live `/github` inbox push (docs §6.1).
///
/// Each upsert sends a small, credential-free [`ServerEvent::GithubEvent`] on
/// success so the webui refreshes without polling. Held by
/// [`crate::GithubState`] and passed into the store functions. `cctui-github`
/// depends only on `cctui-proto` (which owns `ServerEvent`), never on
/// `cctui-server`.
pub type EventTx = broadcast::Sender<ServerEvent>;

/// Send a "something changed" nudge for a just-upserted GitHub object.
///
/// Best-effort: a send error just means no client is currently subscribed,
/// which is fine — the inbox reconciles on its next subscribe. The locator is
/// credential-free (no token, no row body, no raw payload), so nothing
/// sensitive crosses the wire.
fn broadcast_event(
    events: &EventTx,
    kind: GithubEventKind,
    connector_id: Uuid,
    repo: &str,
    pull_number: Option<i64>,
) {
    let _ = events.send(ServerEvent::GithubEvent {
        kind,
        payload: GithubEventPayload { connector_id, repo: repo.to_string(), pull_number },
    });
}

/// Parse an ISO-8601 timestamp from a parsed GitHub object. GitHub always sends
/// RFC-3339, so a parse failure means the upstream parser handed us garbage —
/// surface it as an error rather than silently storing a wrong time.
fn parse_ts(s: &str) -> Result<chrono::DateTime<chrono::Utc>, sqlx::Error> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|t| t.with_timezone(&chrono::Utc))
        .map_err(|e| sqlx::Error::Decode(format!("invalid github timestamp {s:?}: {e}").into()))
}

fn parse_opt_ts(s: Option<&str>) -> Result<Option<chrono::DateTime<chrono::Utc>>, sqlx::Error> {
    s.map(parse_ts).transpose()
}

/// Upsert a synced pull request into `github.pulls`, keyed on
/// `(connector_id, repo, number)`. Returns the row's primary key.
///
/// # Errors
/// Returns an error on a timestamp parse failure or a database error.
pub async fn upsert_pull(
    pool: &PgPool,
    events: &EventTx,
    connector_id: Uuid,
    p: &PullUpsert,
) -> Result<Uuid, sqlx::Error> {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO github.pulls \
            (connector_id, node_id, repo, number, title, state, merged, draft, \
             mergeable_state, author, head_sha, base_ref, head_ref, \
             gh_created_at, gh_updated_at, synced_at) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15, now()) \
         ON CONFLICT (connector_id, repo, number) DO UPDATE SET \
             node_id = EXCLUDED.node_id, title = EXCLUDED.title, \
             state = EXCLUDED.state, merged = EXCLUDED.merged, \
             draft = EXCLUDED.draft, mergeable_state = EXCLUDED.mergeable_state, \
             author = EXCLUDED.author, head_sha = EXCLUDED.head_sha, \
             base_ref = EXCLUDED.base_ref, head_ref = EXCLUDED.head_ref, \
             gh_created_at = EXCLUDED.gh_created_at, \
             gh_updated_at = EXCLUDED.gh_updated_at, synced_at = now() \
         RETURNING id",
    )
    .bind(connector_id)
    .bind(&p.node_id)
    .bind(&p.repo)
    .bind(p.number)
    .bind(&p.title)
    .bind(&p.state)
    .bind(p.merged)
    .bind(p.draft)
    .bind(&p.mergeable_state)
    .bind(&p.author)
    .bind(&p.head_sha)
    .bind(&p.base_ref)
    .bind(&p.head_ref)
    .bind(parse_ts(&p.gh_created_at)?)
    .bind(parse_ts(&p.gh_updated_at)?)
    .fetch_one(pool)
    .await?;
    broadcast_event(events, GithubEventKind::Pull, connector_id, &p.repo, Some(p.number));
    Ok(id)
}

/// Upsert a CI check into `github.checks`, keyed on
/// `(connector_id, repo, head_sha, external_id)`. Returns the row's primary key.
///
/// # Errors
/// Returns an error on a database error.
pub async fn upsert_check(
    pool: &PgPool,
    events: &EventTx,
    connector_id: Uuid,
    c: &CheckUpsert,
) -> Result<Uuid, sqlx::Error> {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO github.checks \
            (connector_id, repo, head_sha, external_id, name, status, \
             conclusion, details_url, synced_at) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8, now()) \
         ON CONFLICT (connector_id, repo, head_sha, external_id) DO UPDATE SET \
             name = EXCLUDED.name, status = EXCLUDED.status, \
             conclusion = EXCLUDED.conclusion, details_url = EXCLUDED.details_url, \
             synced_at = now() \
         RETURNING id",
    )
    .bind(connector_id)
    .bind(&c.repo)
    .bind(&c.head_sha)
    .bind(&c.external_id)
    .bind(&c.name)
    .bind(&c.status)
    .bind(&c.conclusion)
    .bind(&c.details_url)
    .fetch_one(pool)
    .await?;
    // Checks are keyed on a head SHA, not a PR number; the client maps the SHA
    // back to a PR via its cache, so `pull_number` is `None` here.
    broadcast_event(events, GithubEventKind::Check, connector_id, &c.repo, None);
    Ok(id)
}

/// Upsert a submitted review into `github.reviews`, keyed on
/// `(connector_id, review_id)`. Returns the row's primary key.
///
/// # Errors
/// Returns an error on a timestamp parse failure or a database error.
pub async fn upsert_review(
    pool: &PgPool,
    events: &EventTx,
    connector_id: Uuid,
    r: &ReviewUpsert,
) -> Result<Uuid, sqlx::Error> {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO github.reviews \
            (connector_id, repo, pull_number, review_id, reviewer, state, \
             body, commit_id, submitted_at, synced_at) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9, now()) \
         ON CONFLICT (connector_id, review_id) DO UPDATE SET \
             repo = EXCLUDED.repo, pull_number = EXCLUDED.pull_number, \
             reviewer = EXCLUDED.reviewer, state = EXCLUDED.state, \
             body = EXCLUDED.body, commit_id = EXCLUDED.commit_id, \
             submitted_at = EXCLUDED.submitted_at, synced_at = now() \
         RETURNING id",
    )
    .bind(connector_id)
    .bind(&r.repo)
    .bind(r.pull_number)
    .bind(r.review_id)
    .bind(&r.reviewer)
    .bind(&r.state)
    .bind(&r.body)
    .bind(&r.commit_id)
    .bind(parse_opt_ts(r.submitted_at.as_deref())?)
    .fetch_one(pool)
    .await?;
    broadcast_event(events, GithubEventKind::Review, connector_id, &r.repo, Some(r.pull_number));
    Ok(id)
}

/// Upsert a review thread into `github.review_threads`, keyed on
/// `(connector_id, thread_node_id)`. Returns the row's primary key.
///
/// # Errors
/// Returns an error on a database error.
pub async fn upsert_review_thread(
    pool: &PgPool,
    events: &EventTx,
    connector_id: Uuid,
    t: &ReviewThreadUpsert,
) -> Result<Uuid, sqlx::Error> {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO github.review_threads \
            (connector_id, repo, pull_number, thread_node_id, path, side, \
             line, resolved, synced_at) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8, now()) \
         ON CONFLICT (connector_id, thread_node_id) DO UPDATE SET \
             repo = EXCLUDED.repo, pull_number = EXCLUDED.pull_number, \
             path = EXCLUDED.path, side = EXCLUDED.side, line = EXCLUDED.line, \
             resolved = EXCLUDED.resolved, synced_at = now() \
         RETURNING id",
    )
    .bind(connector_id)
    .bind(&t.repo)
    .bind(t.pull_number)
    .bind(&t.thread_node_id)
    .bind(&t.path)
    .bind(&t.side)
    .bind(t.line)
    .bind(t.resolved)
    .fetch_one(pool)
    .await?;
    broadcast_event(
        events,
        GithubEventKind::ReviewThread,
        connector_id,
        &t.repo,
        Some(t.pull_number),
    );
    Ok(id)
}

/// Upsert a review comment into `github.review_comments`, keyed on
/// `(connector_id, comment_id)`. Returns the row's primary key.
///
/// # Errors
/// Returns an error on a timestamp parse failure or a database error.
pub async fn upsert_review_comment(
    pool: &PgPool,
    events: &EventTx,
    connector_id: Uuid,
    c: &ReviewCommentUpsert,
) -> Result<Uuid, sqlx::Error> {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO github.review_comments \
            (connector_id, repo, pull_number, comment_id, thread_node_id, \
             author, body, path, side, line, gh_created_at, gh_updated_at, \
             synced_at) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12, now()) \
         ON CONFLICT (connector_id, comment_id) DO UPDATE SET \
             repo = EXCLUDED.repo, pull_number = EXCLUDED.pull_number, \
             thread_node_id = EXCLUDED.thread_node_id, author = EXCLUDED.author, \
             body = EXCLUDED.body, path = EXCLUDED.path, side = EXCLUDED.side, \
             line = EXCLUDED.line, gh_created_at = EXCLUDED.gh_created_at, \
             gh_updated_at = EXCLUDED.gh_updated_at, synced_at = now() \
         RETURNING id",
    )
    .bind(connector_id)
    .bind(&c.repo)
    .bind(c.pull_number)
    .bind(c.comment_id)
    .bind(&c.thread_node_id)
    .bind(&c.author)
    .bind(&c.body)
    .bind(&c.path)
    .bind(&c.side)
    .bind(c.line)
    .bind(parse_ts(&c.gh_created_at)?)
    .bind(parse_ts(&c.gh_updated_at)?)
    .fetch_one(pool)
    .await?;
    broadcast_event(
        events,
        GithubEventKind::ReviewComment,
        connector_id,
        &c.repo,
        Some(c.pull_number),
    );
    Ok(id)
}

/// List the PR's OPEN (unresolved) pulled-down GitHub review threads + their
/// comments, scoped to a connector.
///
/// Read side of GH-VIEW-5's pull-down: the
/// webui renders these inline alongside local drafts (visually distinct).
///
/// Threads are ordered by `(path, line)` so they group by file in the viewer;
/// comments within a thread are oldest-first.
///
/// # Errors
/// Returns an error on a database error.
pub async fn list_open_threads(
    pool: &PgPool,
    connector_id: Uuid,
    repo: &str,
    number: i64,
) -> Result<Vec<cctui_proto::github::ReviewThreadInfo>, sqlx::Error> {
    use sqlx::Row;

    let thread_rows = sqlx::query(
        "SELECT thread_node_id, path, side, line, resolved \
         FROM github.review_threads \
         WHERE connector_id = $1 AND repo = $2 AND pull_number = $3 AND resolved = FALSE \
         ORDER BY path, line",
    )
    .bind(connector_id)
    .bind(repo)
    .bind(number)
    .fetch_all(pool)
    .await?;

    let mut threads = Vec::with_capacity(thread_rows.len());
    for row in &thread_rows {
        let thread_node_id: String = row.try_get("thread_node_id")?;
        let comments = sqlx::query(
            "SELECT comment_id, author, body, gh_created_at \
             FROM github.review_comments \
             WHERE connector_id = $1 AND thread_node_id = $2 \
             ORDER BY gh_created_at, comment_id",
        )
        .bind(connector_id)
        .bind(&thread_node_id)
        .fetch_all(pool)
        .await?
        .iter()
        .map(|c| {
            Ok::<_, sqlx::Error>(cctui_proto::github::ReviewThreadCommentInfo {
                comment_id: c.try_get("comment_id")?,
                author: c.try_get("author")?,
                body: c.try_get("body")?,
                created_at: c
                    .try_get::<chrono::DateTime<chrono::Utc>, _>("gh_created_at")?
                    .to_rfc3339(),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

        threads.push(cctui_proto::github::ReviewThreadInfo {
            thread_node_id,
            path: row.try_get("path")?,
            side: row.try_get("side")?,
            line: row.try_get("line")?,
            resolved: row.try_get("resolved")?,
            comments,
        });
    }
    Ok(threads)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_pull() -> PullUpsert {
        PullUpsert {
            node_id: "PR_node".into(),
            repo: "o/r".into(),
            number: 7,
            title: "t".into(),
            state: "open".into(),
            merged: false,
            draft: false,
            mergeable_state: Some("clean".into()),
            author: "me".into(),
            head_sha: "abc".into(),
            base_ref: "main".into(),
            head_ref: "feat".into(),
            gh_created_at: "2026-06-17T00:00:00Z".into(),
            gh_updated_at: "2026-06-17T01:00:00Z".into(),
        }
    }

    #[test]
    fn parse_ts_accepts_rfc3339() {
        assert!(parse_ts("2026-06-17T00:00:00Z").is_ok());
        assert!(parse_ts("2026-06-17T00:00:00+09:00").is_ok());
    }

    #[test]
    fn parse_ts_rejects_garbage() {
        assert!(parse_ts("not-a-time").is_err());
        assert!(parse_ts("2026-06-17").is_err());
    }

    #[test]
    fn parse_opt_ts_handles_none_and_some() {
        assert!(matches!(parse_opt_ts(None), Ok(None)));
        assert!(matches!(parse_opt_ts(Some("2026-06-17T00:00:00Z")), Ok(Some(_))));
        assert!(parse_opt_ts(Some("garbage")).is_err());
    }

    #[test]
    fn sample_pull_timestamps_parse() {
        let p = sample_pull();
        assert!(parse_ts(&p.gh_created_at).is_ok());
        assert!(parse_ts(&p.gh_updated_at).is_ok());
    }
}
