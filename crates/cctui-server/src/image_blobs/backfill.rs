//! Off unless `CCTUI_IMAGE_BACKFILL=1`; knobs in `docs/image-blobs.md`.

use std::time::Duration;

use serde_json::Value;
use sqlx::{Connection, PgPool};

pub const NAME: &str = "backfill";

const DEFAULT_SCAN: i64 = 500;
const DEFAULT_ROWS: usize = 20;
const DEFAULT_PAUSE: Duration = Duration::from_secs(1);
const DEFAULT_BUDGET_MB: u64 = 512;
const IDLE: Duration = Duration::from_mins(1);
const ERROR_BACKOFF: Duration = Duration::from_mins(1);
/// Ids are drawn before their transaction commits; the walk stays behind rows
/// this recent so it does not pass one that is not visible yet.
const COMMIT_LAG: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    pub scan: i64,
    pub rows: usize,
    pub pause: Duration,
    pub budget_bytes: u64,
}

impl Config {
    #[must_use]
    pub fn from_env() -> Option<Self> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    #[must_use]
    pub(super) fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Option<Self> {
        let on = get("CCTUI_IMAGE_BACKFILL")
            .map(|v| v.trim().to_ascii_lowercase())
            .is_some_and(|v| matches!(v.as_str(), "1" | "true" | "on" | "yes"));
        if !on {
            return None;
        }
        let num = |k: &str| get(k).and_then(|v| v.trim().parse::<u64>().ok()).filter(|n| *n > 0);
        Some(Self {
            scan: num("CCTUI_IMAGE_BACKFILL_SCAN")
                .and_then(|n| i64::try_from(n).ok())
                .unwrap_or(DEFAULT_SCAN),
            rows: num("CCTUI_IMAGE_BACKFILL_ROWS")
                .and_then(|n| usize::try_from(n).ok())
                .unwrap_or(DEFAULT_ROWS),
            pause: num("CCTUI_IMAGE_BACKFILL_PAUSE_MS")
                .map_or(DEFAULT_PAUSE, Duration::from_millis),
            budget_bytes: num("CCTUI_IMAGE_BACKFILL_BUDGET_MB")
                .unwrap_or(DEFAULT_BUDGET_MB)
                .saturating_mul(1024 * 1024),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Rewritten { blob_bytes: u64 },
    Nothing,
    Gone,
    Changed,
    Duplicate,
    BadBlob,
}

/// Any error rolls the blob writes back and leaves the row untouched.
pub async fn rewrite_event(conn: &mut sqlx::PgConnection, id: i64) -> Result<Outcome, sqlx::Error> {
    let row: Option<(String, Value, Vec<u8>)> =
        sqlx::query_as("SELECT session_id, payload, content_hash FROM stream_events WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some((session_id, payload, content_hash)) = row else {
        return Ok(Outcome::Gone);
    };
    let Some((rewritten, blobs)) = super::extract(&payload) else {
        return Ok(Outcome::Nothing);
    };
    drop(payload);
    let mut tx = conn.begin().await?;
    let Some(blob_bytes) = super::store_verified(&mut tx, &blobs).await? else {
        return Ok(Outcome::BadBlob);
    };
    let hashes: Vec<String> = blobs.into_iter().map(|b| b.hash).collect();
    sqlx::query(
        "INSERT INTO session_blob_links (session_id, hash) \
         SELECT $1, * FROM UNNEST($2::text[]) ON CONFLICT DO NOTHING",
    )
    .bind(&session_id)
    .bind(&hashes)
    .execute(&mut *tx)
    .await?;
    let updated =
        sqlx::query("UPDATE stream_events SET payload = $2 WHERE id = $1 AND content_hash = $3")
            .bind(id)
            .bind(&rewritten)
            .bind(&content_hash)
            .execute(&mut *tx)
            .await;
    match updated {
        Ok(r) if r.rows_affected() == 1 => {
            tx.commit().await?;
            Ok(Outcome::Rewritten { blob_bytes })
        }
        Ok(_) => Ok(Outcome::Changed),
        Err(sqlx::Error::Database(e)) if e.code().as_deref() == Some("23505") => {
            Ok(Outcome::Duplicate)
        }
        Err(e) => Err(e),
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    pub rewritten: usize,
    pub left_inline: usize,
    pub blob_bytes: u64,
    pub more: bool,
}

const SCAN_SQL: &str = "WITH scan AS ( \
         SELECT id FROM stream_events \
         WHERE id > $1 AND id <= $2 AND ($5::text[] IS NULL OR session_id = ANY($5)) \
         ORDER BY id LIMIT $3) \
     SELECT (SELECT max(id) FROM scan), \
            ARRAY(SELECT e.id FROM scan s JOIN stream_events e ON e.id = s.id \
                  WHERE CASE WHEN pg_column_size(e.payload) >= $4 \
                             THEN strpos(e.payload::text, '\"type\": \"base64\"') > 0 \
                             ELSE false END \
                  ORDER BY e.id)";

pub struct Walk {
    pub name: String,
    pub scan: i64,
    pub rows: usize,
    pub lag: Duration,
    pub scope: Option<Vec<String>>,
}

impl Walk {
    #[must_use]
    pub fn new(config: &Config) -> Self {
        Self {
            name: NAME.to_owned(),
            scan: config.scan,
            rows: config.rows.max(1),
            lag: COMMIT_LAG,
            scope: None,
        }
    }

    async fn ceiling(&self, pool: &PgPool) -> Result<Option<i64>, sqlx::Error> {
        sqlx::query_scalar(
            "SELECT id FROM stream_events WHERE created_at < now() - make_interval(secs => $1) \
             ORDER BY id DESC LIMIT 1",
        )
        .bind(self.lag.as_secs_f64())
        .fetch_optional(pool)
        .await
    }

    /// A row that errors aborts the batch before the cursor moves, so the next
    /// batch retries it.
    pub async fn step(&self, pool: &PgPool) -> Result<Step, sqlx::Error> {
        let Some(ceiling) = self.ceiling(pool).await? else {
            return Ok(Step::default());
        };
        let mut lock = pool.begin().await?;
        let locked: bool = sqlx::query_scalar(
            "SELECT pg_try_advisory_xact_lock(hashtext('image_blob_backfill/' || $1))",
        )
        .bind(&self.name)
        .fetch_one(&mut *lock)
        .await?;
        if !locked {
            return Ok(Step::default());
        }
        let cursor: i64 =
            sqlx::query_scalar("SELECT last_event_id FROM image_blob_backfill WHERE name = $1")
                .bind(&self.name)
                .fetch_optional(&mut *lock)
                .await?
                .unwrap_or(0);
        if cursor >= ceiling {
            return Ok(Step::default());
        }
        let (last, ids): (Option<i64>, Vec<i64>) = sqlx::query_as(SCAN_SQL)
            .bind(cursor)
            .bind(ceiling)
            .bind(self.scan)
            .bind(i32::try_from(super::MIN_BLOB_BYTES).unwrap_or(i32::MAX))
            .bind(self.scope.as_deref())
            .fetch_one(&mut *lock)
            .await?;
        let (todo, next) = if ids.len() > self.rows {
            (&ids[..self.rows], ids[self.rows - 1])
        } else {
            (&ids[..], last.unwrap_or(ceiling))
        };
        let mut step = Step { more: next < ceiling, ..Step::default() };
        let mut conn = pool.acquire().await?;
        for &id in todo {
            match rewrite_event(&mut conn, id).await? {
                Outcome::Rewritten { blob_bytes } => {
                    step.rewritten += 1;
                    step.blob_bytes += blob_bytes;
                }
                Outcome::Nothing | Outcome::Gone => {}
                other => {
                    tracing::info!(id, ?other, "image backfill left an event inline");
                    step.left_inline += 1;
                }
            }
        }
        sqlx::query(
            "INSERT INTO image_blob_backfill AS t \
             (name, last_event_id, rows_rewritten, blob_bytes) VALUES ($1, $2, $3, $4) \
             ON CONFLICT (name) DO UPDATE SET last_event_id = EXCLUDED.last_event_id, \
             rows_rewritten = t.rows_rewritten + EXCLUDED.rows_rewritten, \
             blob_bytes = t.blob_bytes + EXCLUDED.blob_bytes, updated_at = now()",
        )
        .bind(&self.name)
        .bind(next)
        .bind(i64::try_from(step.rewritten).unwrap_or(i64::MAX))
        .bind(i64::try_from(step.blob_bytes).unwrap_or(i64::MAX))
        .execute(&mut *lock)
        .await?;
        lock.commit().await?;
        Ok(step)
    }
}

/// Stops once this process has written `budget_bytes` of new blobs: the old
/// rows keep their space until vacuum reclaims it.
pub async fn task(pool: PgPool, config: Config) {
    let walk = Walk::new(&config);
    let mut spent = 0u64;
    tracing::info!(budget_bytes = config.budget_bytes, "image blob backfill enabled");
    loop {
        let wait = match walk.step(&pool).await {
            Ok(step) => {
                spent = spent.saturating_add(step.blob_bytes);
                if step.rewritten + step.left_inline > 0 {
                    tracing::info!(
                        rewritten = step.rewritten,
                        left_inline = step.left_inline,
                        blob_bytes = step.blob_bytes,
                        spent,
                        "image blob backfill batch"
                    );
                }
                if spent >= config.budget_bytes {
                    tracing::warn!(spent, "image blob backfill stopped: byte budget reached");
                    return;
                }
                if step.more { config.pause } else { IDLE }
            }
            Err(err) => {
                tracing::warn!(%err, "image blob backfill batch failed");
                ERROR_BACKOFF
            }
        };
        tokio::time::sleep(wait).await;
    }
}
