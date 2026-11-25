use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{PgConnection, PgPool};
use tokio::time::Instant;

use super::TOOL_RESULT;
use super::lifecycle::{self, Rule};
use crate::context::SearchReindex;

pub const NAME: &str = "default";

const SCAN: i64 = 1000;
const PAUSE: Duration = Duration::from_millis(500);
const POLL: Duration = Duration::from_mins(1);
const IDLE: Duration = Duration::from_hours(1);
const ERROR_BACKOFF: Duration = Duration::from_mins(5);

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    pub scanned: u64,
    pub trimmed: u64,
    pub dropped: u64,
    pub skipped: u64,
    pub freed: i64,
    pub more: bool,
}

pub struct Walk {
    pub name: String,
    pub rules: Vec<Rule>,
    pub scan: i64,
    pub scope: Option<Vec<String>>,
}

type Candidate = (i64, String, Value, Vec<u8>, i32);

impl Walk {
    pub fn new(name: &str, rules: Vec<Rule>) -> Self {
        Self { name: name.to_owned(), rules, scan: SCAN, scope: None }
    }

    async fn ceiling(
        &self,
        pool: &PgPool,
        cutoff: DateTime<Utc>,
    ) -> Result<Option<i64>, sqlx::Error> {
        match &self.scope {
            Some(scope) => {
                sqlx::query_scalar(
                    "SELECT max(id) FROM stream_events WHERE session_id = ANY($1) \
                     AND created_at < $2",
                )
                .bind(scope.as_slice())
                .bind(cutoff)
                .fetch_one(pool)
                .await
            }
            None => ceiling_by_search(pool, cutoff).await,
        }
    }

    pub async fn step(
        &self,
        pool: &PgPool,
        search: &impl SearchReindex,
    ) -> Result<Step, sqlx::Error> {
        let mut tx = pool.begin().await?;
        if !locked(&mut tx, &self.name).await? {
            tx.rollback().await?;
            return Ok(Step::default());
        }
        for &rule in &self.rules {
            let cutoff = Utc::now() - chrono::Duration::days(i64::from(rule.after_days()));
            let Some(ceiling) = self.ceiling(pool, cutoff).await? else { continue };
            let cursor = cursor(&mut tx, &self.name, rule).await?;
            if cursor >= ceiling {
                continue;
            }
            let (next, step, updated) = self.batch(&mut tx, rule, cursor, ceiling, cutoff).await?;
            save(&mut tx, &self.name, rule, next, &step).await?;
            tx.commit().await?;
            if step.trimmed + step.dropped > 0 {
                tracing::info!(
                    action = rule.action(),
                    cursor = next,
                    trimmed = step.trimmed,
                    dropped = step.dropped,
                    skipped = step.skipped,
                    freed = step.freed,
                    "storage lifecycle batch"
                );
            }
            reindex(pool, search, &updated).await;
            return Ok(Step { more: true, ..step });
        }
        finish(&mut tx, &self.name).await?;
        tx.commit().await?;
        Ok(Step::default())
    }

    async fn batch(
        &self,
        conn: &mut PgConnection,
        rule: Rule,
        cursor: i64,
        ceiling: i64,
        cutoff: DateTime<Utc>,
    ) -> Result<(i64, Step, Vec<i64>), sqlx::Error> {
        let end: Option<i64> = sqlx::query_scalar(
            "SELECT max(id) FROM (SELECT id FROM stream_events WHERE id > $1 AND id <= $2 \
             AND ($4::text[] IS NULL OR session_id = ANY($4)) ORDER BY id LIMIT $3) w",
        )
        .bind(cursor)
        .bind(ceiling)
        .bind(self.scan)
        .bind(self.scope.as_deref())
        .fetch_one(&mut *conn)
        .await?;
        let end = end.unwrap_or(ceiling);
        let rows: Vec<Candidate> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT e.id, e.event_type, e.payload, e.content_hash, pg_column_size(e.payload) \
             FROM stream_events e WHERE e.id > $1 AND e.id <= $2 AND e.created_at < $3 \
             AND ($4::text[] IS NULL OR e.session_id = ANY($4)) AND {TOOL_RESULT} ORDER BY e.id"
        )))
        .bind(cursor)
        .bind(end)
        .bind(cutoff)
        .bind(self.scope.as_deref())
        .fetch_all(&mut *conn)
        .await?;
        let mut step = Step { scanned: rows.len() as u64, ..Step::default() };
        let (mut updated, mut before) = (Vec::new(), 0i64);
        for (id, event_type, payload, hash, size) in rows {
            let Some(next) = lifecycle::apply(rule, &event_type, &payload) else { continue };
            if rewrite(conn, id, &next, &hash).await? {
                updated.push(id);
                before += i64::from(size);
            } else {
                step.skipped += 1;
            }
        }
        let after: i64 = sqlx::query_scalar(
            "SELECT COALESCE(sum(pg_column_size(payload)), 0)::int8 FROM stream_events \
             WHERE id = ANY($1)",
        )
        .bind(&updated)
        .fetch_one(&mut *conn)
        .await?;
        let changed = updated.len() as u64;
        match rule {
            Rule::Trim { .. } => step.trimmed = changed,
            Rule::Drop { .. } => step.dropped = changed,
        }
        step.freed = (before - after).max(0);
        Ok((end, step, updated))
    }
}

/// Bisects the primary key: no index covers `created_at` alone, and ids grow with it.
async fn ceiling_by_search(
    pool: &PgPool,
    cutoff: DateTime<Utc>,
) -> Result<Option<i64>, sqlx::Error> {
    let at = |id: i64| {
        sqlx::query_as::<_, (i64, DateTime<Utc>)>(
            "SELECT id, created_at FROM stream_events WHERE id >= $1 ORDER BY id LIMIT 1",
        )
        .bind(id)
        .fetch_optional(pool)
    };
    let Some((mut lo, first)) = at(i64::MIN).await? else { return Ok(None) };
    if first >= cutoff {
        return Ok(None);
    }
    let (mut hi, last): (i64, DateTime<Utc>) =
        sqlx::query_as("SELECT id, created_at FROM stream_events ORDER BY id DESC LIMIT 1")
            .fetch_one(pool)
            .await?;
    if last < cutoff {
        return Ok(Some(hi));
    }
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        match at(mid).await? {
            Some((id, created)) if id < hi && created < cutoff => lo = id,
            _ => hi = mid,
        }
    }
    Ok(Some(lo))
}

/// Skips a row edited since it was read, and a rewrite that would collide on the
/// ingest dedup's content hash.
async fn rewrite(
    conn: &mut PgConnection,
    id: i64,
    payload: &Value,
    hash: &[u8],
) -> Result<bool, sqlx::Error> {
    let done = sqlx::query(
        "UPDATE stream_events e SET payload = $2 WHERE e.id = $1 AND e.content_hash = $3 \
         AND NOT EXISTS (SELECT 1 FROM stream_events o WHERE o.session_id = e.session_id \
           AND o.event_type = e.event_type AND o.id <> e.id \
           AND o.content_hash = public.digest(($2::jsonb)::text, 'sha256'))",
    )
    .bind(id)
    .bind(payload)
    .bind(hash)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

async fn locked(conn: &mut PgConnection, name: &str) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtext('storage_lifecycle/' || $1))")
        .bind(name)
        .fetch_one(conn)
        .await
}

async fn cursor(conn: &mut PgConnection, name: &str, rule: Rule) -> Result<i64, sqlx::Error> {
    let row: Option<(String, i64)> = sqlx::query_as(
        "SELECT rule, last_event_id FROM storage_lifecycle_cursors WHERE name = $1 AND action = $2",
    )
    .bind(name)
    .bind(rule.action())
    .fetch_optional(conn)
    .await?;
    Ok(row.filter(|(print, _)| *print == rule.fingerprint()).map_or(0, |(_, id)| id))
}

async fn save(
    conn: &mut PgConnection,
    name: &str,
    rule: Rule,
    next: i64,
    step: &Step,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO storage_lifecycle_cursors (name, action, rule, last_event_id) \
         VALUES ($1, $2, $3, $4) ON CONFLICT (name, action) DO UPDATE \
         SET rule = EXCLUDED.rule, last_event_id = EXCLUDED.last_event_id, updated_at = now()",
    )
    .bind(name)
    .bind(rule.action())
    .bind(rule.fingerprint())
    .bind(next)
    .execute(&mut *conn)
    .await?;
    let count = |n: u64| i64::try_from(n).unwrap_or(i64::MAX);
    sqlx::query(
        "WITH open AS (SELECT id FROM storage_lifecycle_runs \
                       WHERE name = $1 AND finished_at IS NULL ORDER BY id DESC LIMIT 1), \
         bumped AS (UPDATE storage_lifecycle_runs r SET rows_scanned = r.rows_scanned + $2, \
                      rows_trimmed = r.rows_trimmed + $3, rows_dropped = r.rows_dropped + $4, \
                      rows_skipped = r.rows_skipped + $5, bytes_freed = r.bytes_freed + $6 \
                    FROM open WHERE r.id = open.id RETURNING r.id) \
         INSERT INTO storage_lifecycle_runs (name, rows_scanned, rows_trimmed, rows_dropped, \
           rows_skipped, bytes_freed) \
         SELECT $1, $2, $3, $4, $5, $6 WHERE NOT EXISTS (SELECT 1 FROM bumped)",
    )
    .bind(name)
    .bind(count(step.scanned))
    .bind(count(step.trimmed))
    .bind(count(step.dropped))
    .bind(count(step.skipped))
    .bind(step.freed)
    .execute(conn)
    .await?;
    Ok(())
}

type Totals = (i64, i64, i64, i64, i64);

async fn finish(conn: &mut PgConnection, name: &str) -> Result<(), sqlx::Error> {
    let closed: Option<Totals> = sqlx::query_as(
        "UPDATE storage_lifecycle_runs SET finished_at = now() \
         WHERE name = $1 AND finished_at IS NULL \
         RETURNING rows_scanned, rows_trimmed, rows_dropped, rows_skipped, bytes_freed",
    )
    .bind(name)
    .fetch_optional(conn)
    .await?;
    if let Some((scanned, trimmed, dropped, skipped, freed)) = closed {
        tracing::info!(scanned, trimmed, dropped, skipped, freed, "storage lifecycle run finished");
    }
    Ok(())
}

async fn note_error(pool: &PgPool, name: &str, err: &sqlx::Error) {
    let noted = sqlx::query(
        "WITH open AS (SELECT id FROM storage_lifecycle_runs \
                       WHERE name = $1 AND finished_at IS NULL ORDER BY id DESC LIMIT 1), \
         marked AS (UPDATE storage_lifecycle_runs r SET error = $2 FROM open \
                    WHERE r.id = open.id RETURNING r.id) \
         INSERT INTO storage_lifecycle_runs (name, finished_at, error) \
         SELECT $1, now(), $2 WHERE NOT EXISTS (SELECT 1 FROM marked)",
    )
    .bind(name)
    .bind(err.to_string())
    .execute(pool)
    .await;
    if let Err(e) = noted {
        tracing::warn!(%e, "storage lifecycle error not recorded");
    }
}

/// With search off `search_chunks` stays empty, so only indexed events are rebuilt.
async fn reindex(pool: &PgPool, search: &impl SearchReindex, ids: &[i64]) {
    if ids.is_empty() {
        return;
    }
    let rebuilt = async {
        let chunked: Vec<i64> = sqlx::query_scalar(
            "SELECT DISTINCT event_id FROM search_chunks WHERE event_id = ANY($1)",
        )
        .bind(ids)
        .fetch_all(pool)
        .await?;
        search.reindex_search(pool, &chunked).await
    };
    if let Err(err) = rebuilt.await {
        tracing::warn!(%err, "storage lifecycle left stale search chunks");
    }
}

/// With no rule enabled this only reads the rules table, once a minute.
pub async fn task(pool: PgPool, search: impl SearchReindex) {
    let mut walked: Option<(Vec<Rule>, Instant)> = None;
    loop {
        let wait = match lifecycle::load_rules(&pool).await.map(|r| lifecycle::active(&r)) {
            Ok(rules) if rules.is_empty() => {
                walked = None;
                POLL
            }
            Ok(rules) => tick(&pool, &search, rules, &mut walked).await,
            Err(err) => {
                tracing::warn!(%err, "storage lifecycle rules unreadable");
                POLL
            }
        };
        tokio::time::sleep(wait).await;
    }
}

async fn tick(
    pool: &PgPool,
    search: &impl SearchReindex,
    rules: Vec<Rule>,
    walked: &mut Option<(Vec<Rule>, Instant)>,
) -> Duration {
    let rested =
        walked.as_ref().is_some_and(|(seen, until)| *seen == rules && Instant::now() < *until);
    if rested {
        return POLL;
    }
    let walk = Walk::new(NAME, rules.clone());
    match walk.step(pool, search).await {
        Ok(step) if step.more => {
            *walked = None;
            PAUSE
        }
        Ok(_) => {
            *walked = Some((rules, Instant::now() + IDLE));
            POLL
        }
        Err(err) => {
            tracing::warn!(%err, "storage lifecycle batch failed");
            note_error(pool, NAME, &err).await;
            *walked = Some((rules, Instant::now() + ERROR_BACKOFF));
            POLL
        }
    }
}
