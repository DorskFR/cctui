//! Daemon events are chunked on ingest; every other row, history included, is
//! reached by the backfill, which `CCTUI_SEARCH_BACKFILL=0` pauses in place.

pub mod text;

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::Value;
use sqlx::PgPool;

/// The `search_index_state` row of the production backfill. Its advisory lock
/// (`hashtext(name)`) also serialises [`reindex`] against a running batch.
pub const BACKFILL: &str = "backfill";
/// The `search_index_state` row holding the first event id chunked on ingest.
pub const LIVE_FROM: &str = "live_from";

static LIVE_FROM_RECORDED: AtomicBool = AtomicBool::new(false);

const DEFAULT_BATCH: i64 = 50;
const DEFAULT_PAUSE: Duration = Duration::from_millis(250);
/// Wait between polls once the walk has caught up.
const IDLE: Duration = Duration::from_secs(5);
const ERROR_BACKOFF: Duration = Duration::from_secs(30);
/// Ids are drawn before their transaction commits, so a row can appear below
/// the highest visible id after the walk has passed it. The walk only goes as
/// far as the highest id seen this long ago.
const COMMIT_LAG: Duration = Duration::from_secs(30);

/// One event's chunks, ready to insert.
pub struct Chunked {
    pub event_id: i64,
    pub session_id: String,
    pub bodies: Vec<String>,
}

/// The chunks of one stored event, from the canonical shape the webui renders.
#[must_use]
pub fn chunk_event(adapter_id: &str, event_type: &str, payload: Value) -> Vec<String> {
    crate::normalize::for_client(adapter_id, event_type, payload).map_or_else(Vec::new, |client| {
        let text = text::client_text(&client);
        text::chunks(&text).into_iter().map(str::to_owned).collect()
    })
}

/// Insert chunks, skipping any already present and any whose event is gone.
pub async fn insert_chunks<'e, E>(ex: E, rows: &[Chunked]) -> Result<u64, sqlx::Error>
where
    E: sqlx::PgExecutor<'e>,
{
    let mut ids = Vec::new();
    let mut ords = Vec::new();
    let mut sessions = Vec::new();
    let mut bodies = Vec::new();
    for row in rows {
        for (ord, body) in row.bodies.iter().enumerate() {
            ids.push(row.event_id);
            ords.push(i32::try_from(ord).unwrap_or(i32::MAX));
            sessions.push(row.session_id.as_str());
            bodies.push(body.as_str());
        }
    }
    if ids.is_empty() {
        return Ok(0);
    }
    let done = sqlx::query(
        "INSERT INTO search_chunks (event_id, ord, session_id, body) \
         SELECT u.event_id, u.ord, u.session_id, u.body \
         FROM unnest($1::bigint[], $2::int[], $3::text[], $4::text[]) \
              AS u(event_id, ord, session_id, body) \
         WHERE EXISTS (SELECT 1 FROM stream_events e WHERE e.id = u.event_id FOR KEY SHARE) \
         ON CONFLICT DO NOTHING",
    )
    .bind(&ids)
    .bind(&ords)
    .bind(&sessions)
    .bind(&bodies)
    .execute(ex)
    .await?;
    Ok(done.rows_affected())
}

/// A just-ingested event, chunked off the ingest path.
pub struct NewEvent {
    pub event_id: i64,
    pub session_id: String,
    pub adapter_id: String,
    pub event_type: &'static str,
    pub payload: Value,
}

/// Chunk and insert freshly ingested events in the background; a failure only
/// delays them until the backfill reaches their ids.
pub fn index_new(pool: &PgPool, events: Vec<NewEvent>) {
    if events.is_empty() {
        return;
    }
    let pool = pool.clone();
    tokio::spawn(async move {
        if let Some(first) = events.iter().map(|e| e.event_id).min()
            && let Err(err) = record_live_from(&pool, first).await
        {
            tracing::warn!(%err, "recording the first inline-indexed event failed");
        }
        let rows: Vec<Chunked> = events
            .into_iter()
            .map(|e| Chunked {
                bodies: chunk_event(&e.adapter_id, e.event_type, e.payload),
                event_id: e.event_id,
                session_id: e.session_id,
            })
            .collect();
        if let Err(err) = insert_chunks(&pool, &rows).await {
            tracing::warn!(%err, "inline search indexing failed");
        }
    });
}

/// Written once: a later row would hide the ids between the backfill cursor
/// and the first one actually chunked on ingest.
async fn record_live_from(pool: &PgPool, first: i64) -> Result<(), sqlx::Error> {
    if LIVE_FROM_RECORDED.load(Ordering::Relaxed) {
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO search_index_state (name, last_event_id) VALUES ($1, $2) \
         ON CONFLICT (name) DO NOTHING",
    )
    .bind(LIVE_FROM)
    .bind(first)
    .execute(pool)
    .await?;
    LIVE_FROM_RECORDED.store(true, Ordering::Relaxed);
    Ok(())
}

/// Event ids no chunk covers yet: past the backfill cursor and below the first
/// id chunked on ingest. Search matches them on `stream_events.search_text`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unchunked {
    pub after: i64,
    pub before: i64,
}

impl Unchunked {
    /// No `live_from` yet means nothing past the cursor has been chunked.
    pub const fn between(cursor: i64, live_from: Option<i64>) -> Option<Self> {
        let before = match live_from {
            Some(id) => id,
            None => i64::MAX,
        };
        if before.saturating_sub(cursor) > 1 { Some(Self { after: cursor, before }) } else { None }
    }
}

/// The production [`Unchunked`] range; `None` once the backfill has reached
/// the first inline-indexed event.
pub async fn unchunked<'e, E>(ex: E) -> Result<Option<Unchunked>, sqlx::Error>
where
    E: sqlx::PgExecutor<'e>,
{
    unchunked_of(ex, BACKFILL, LIVE_FROM).await
}

async fn unchunked_of<'e, E>(
    ex: E,
    walk: &str,
    live_from: &str,
) -> Result<Option<Unchunked>, sqlx::Error>
where
    E: sqlx::PgExecutor<'e>,
{
    let (cursor, first): (i64, Option<i64>) = sqlx::query_as(
        "SELECT COALESCE((SELECT last_event_id FROM search_index_state WHERE name = $1), 0), \
                (SELECT last_event_id FROM search_index_state WHERE name = $2)",
    )
    .bind(walk)
    .bind(live_from)
    .fetch_one(ex)
    .await?;
    Ok(Unchunked::between(cursor, first))
}

#[derive(sqlx::FromRow)]
struct SourceRow {
    id: i64,
    session_id: String,
    event_type: String,
    payload: Value,
    adapter_id: Option<String>,
}

const SOURCE_SELECT: &str = "SELECT e.id, e.session_id, e.event_type, e.payload, s.adapter_id \
     FROM stream_events e JOIN sessions s ON s.id = e.session_id";

fn chunk_rows(rows: Vec<SourceRow>) -> Vec<Chunked> {
    rows.into_iter()
        .map(|r| Chunked {
            bodies: chunk_event(
                r.adapter_id.as_deref().unwrap_or("claude-code"),
                &r.event_type,
                r.payload,
            ),
            event_id: r.id,
            session_id: r.session_id,
        })
        .collect()
}

/// Rebuild the chunks of events whose payload was rewritten in place (privacy
/// redaction), so the old text stops matching and stops showing in snippets.
pub async fn reindex(pool: &PgPool, ids: &[i64]) -> Result<(), sqlx::Error> {
    if ids.is_empty() {
        return Ok(());
    }
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
        .bind(BACKFILL)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM search_chunks WHERE event_id = ANY($1)")
        .bind(ids)
        .execute(&mut *tx)
        .await?;
    let rows: Vec<SourceRow> =
        sqlx::query_as(sqlx::AssertSqlSafe(format!("{SOURCE_SELECT} WHERE e.id = ANY($1)")))
            .bind(ids)
            .fetch_all(&mut *tx)
            .await?;
    insert_chunks(&mut *tx, &chunk_rows(rows)).await?;
    tx.commit().await
}

/// `CCTUI_SEARCH_BACKFILL` (on unless `0`/`false`/`off`/`no`),
/// `CCTUI_SEARCH_BACKFILL_BATCH` (events per batch, default 50) and
/// `CCTUI_SEARCH_BACKFILL_PAUSE_MS` (sleep between batches, default 250).
pub struct BackfillConfig {
    pub batch: i64,
    pub pause: Duration,
}

impl BackfillConfig {
    #[must_use]
    pub fn from_env() -> Option<Self> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Option<Self> {
        let off = get("CCTUI_SEARCH_BACKFILL")
            .map(|v| v.trim().to_ascii_lowercase())
            .is_some_and(|v| matches!(v.as_str(), "0" | "false" | "off" | "no"));
        if off {
            return None;
        }
        let num = |k: &str| get(k).and_then(|v| v.trim().parse::<u64>().ok()).filter(|n| *n > 0);
        Some(Self {
            batch: num("CCTUI_SEARCH_BACKFILL_BATCH")
                .and_then(|n| i64::try_from(n).ok())
                .unwrap_or(DEFAULT_BATCH),
            pause: num("CCTUI_SEARCH_BACKFILL_PAUSE_MS")
                .map_or(DEFAULT_PAUSE, Duration::from_millis),
        })
    }
}

/// The highest `stream_events.id` that is at least `lag` old, from periodic
/// samples of `max(id)`.
struct Horizon {
    lag: Duration,
    samples: VecDeque<(Instant, i64)>,
}

impl Horizon {
    const fn new(lag: Duration) -> Self {
        Self { lag, samples: VecDeque::new() }
    }

    fn observe(&mut self, at: Instant, max_id: i64) -> Option<i64> {
        self.samples.push_back((at, max_id));
        let ripe = |t: &Instant| at.saturating_duration_since(*t) >= self.lag;
        while self.samples.len() > 1 && self.samples.get(1).is_some_and(|(t, _)| ripe(t)) {
            self.samples.pop_front();
        }
        self.samples.front().filter(|(t, _)| ripe(t)).map(|(_, id)| *id)
    }
}

/// One resumable walk over `stream_events`, keyed by its `search_index_state`
/// name.
pub struct Backfill {
    name: String,
    batch: i64,
    horizon: Horizon,
}

impl Backfill {
    #[must_use]
    pub fn new(name: &str, batch: i64) -> Self {
        Self { name: name.to_owned(), batch, horizon: Horizon::new(COMMIT_LAG) }
    }

    /// Index the next batch in one short transaction. Returns how many events
    /// it read; 0 when caught up or another replica holds the walk.
    pub async fn step(&mut self, pool: &PgPool) -> Result<usize, sqlx::Error> {
        let max_id: i64 = sqlx::query_scalar("SELECT COALESCE(max(id), 0) FROM stream_events")
            .fetch_one(pool)
            .await?;
        let Some(ceiling) = self.horizon.observe(Instant::now(), max_id) else {
            return Ok(0);
        };
        let mut tx = pool.begin().await?;
        let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtext($1))")
            .bind(&self.name)
            .fetch_one(&mut *tx)
            .await?;
        if !locked {
            return Ok(0);
        }
        let cursor: i64 =
            sqlx::query_scalar("SELECT last_event_id FROM search_index_state WHERE name = $1")
                .bind(&self.name)
                .fetch_optional(&mut *tx)
                .await?
                .unwrap_or(0);
        if cursor >= ceiling {
            return Ok(0);
        }
        let rows: Vec<SourceRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "{SOURCE_SELECT} WHERE e.id > $1 AND e.id <= $2 ORDER BY e.id LIMIT $3"
        )))
        .bind(cursor)
        .bind(ceiling)
        .bind(self.batch)
        .fetch_all(&mut *tx)
        .await?;
        let read = rows.len();
        let next = if usize::try_from(self.batch).is_ok_and(|b| read < b) {
            ceiling
        } else {
            rows.last().map_or(ceiling, |r| r.id)
        };
        insert_chunks(&mut *tx, &chunk_rows(rows)).await?;
        sqlx::query(
            "INSERT INTO search_index_state (name, last_event_id) VALUES ($1, $2) \
             ON CONFLICT (name) DO UPDATE SET last_event_id = EXCLUDED.last_event_id, \
             updated_at = now()",
        )
        .bind(&self.name)
        .bind(next)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(read)
    }
}

/// The production backfill loop: a batch, a pause, and a slow poll once it
/// has caught up so new rows from every insert path are reached too.
pub async fn task(pool: PgPool, config: BackfillConfig) {
    let mut walk = Backfill::new(BACKFILL, config.batch);
    let full = usize::try_from(config.batch).unwrap_or(usize::MAX);
    loop {
        let wait = match walk.step(&pool).await {
            Ok(n) if n >= full => config.pause,
            Ok(_) => IDLE,
            Err(err) => {
                tracing::warn!(%err, "search backfill batch failed");
                ERROR_BACKOFF
            }
        };
        tokio::time::sleep(wait).await;
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use uuid::Uuid;

    use super::*;

    #[test]
    fn config_defaults_on_and_can_be_paused() {
        let none = |_: &str| None;
        let on = BackfillConfig::from_lookup(none).expect("on by default");
        assert_eq!((on.batch, on.pause), (DEFAULT_BATCH, DEFAULT_PAUSE));
        for v in ["0", "false", "OFF", " no "] {
            let get = |k: &str| (k == "CCTUI_SEARCH_BACKFILL").then(|| v.to_owned());
            assert!(BackfillConfig::from_lookup(get).is_none(), "{v}");
        }
        let tuned = BackfillConfig::from_lookup(|k: &str| match k {
            "CCTUI_SEARCH_BACKFILL_BATCH" => Some("10".into()),
            "CCTUI_SEARCH_BACKFILL_PAUSE_MS" => Some("1000".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!((tuned.batch, tuned.pause), (10, Duration::from_secs(1)));
    }

    #[test]
    fn horizon_only_releases_ids_seen_long_enough_ago() {
        let t0 = Instant::now();
        let mut h = Horizon::new(Duration::from_secs(30));
        assert_eq!(h.observe(t0, 10), None);
        assert_eq!(h.observe(t0 + Duration::from_secs(10), 20), None);
        assert_eq!(h.observe(t0 + Duration::from_secs(31), 30), Some(10));
        assert_eq!(h.observe(t0 + Duration::from_secs(45), 40), Some(20));
        assert_eq!(h.observe(t0 + Duration::from_secs(100), 50), Some(40));
    }

    #[test]
    fn the_unchunked_range_closes_once_the_cursor_reaches_live_from() {
        assert_eq!(Unchunked::between(10, Some(50)), Some(Unchunked { after: 10, before: 50 }));
        assert_eq!(Unchunked::between(49, Some(50)), None, "no id lies strictly between");
        assert_eq!(Unchunked::between(50, Some(50)), None);
        assert_eq!(Unchunked::between(70, Some(50)), None);
        assert_eq!(
            Unchunked::between(10, None),
            Some(Unchunked { after: 10, before: i64::MAX }),
            "nothing chunked on ingest yet: all of it past the cursor"
        );
    }

    #[test]
    fn image_tool_results_chunk_to_their_text_only() {
        let blob = "iVBORw0KGgoAAAANSUhEUgAA".repeat(40);
        let payload = json!({
            "kind": "tool_result",
            "tool_use_id": "t1",
            "content": [
                {"type": "text", "text": "captured"},
                {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": blob}},
            ],
        });
        assert_eq!(chunk_event("claude-code", "tool_use", payload), vec!["captured".to_owned()]);
    }

    async fn seeded(name: &str) -> Option<(PgPool, String)> {
        let url = crate::routes::gateway::test_db_url(name)?;
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(3)
            .connect(&url)
            .await
            .expect("connect test db");
        let uid = Uuid::new_v4();
        let machine = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("{name}-{uid}"))
            .bind(format!("h-{uid}"))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, 'm', $3)")
            .bind(machine)
            .bind(uid)
            .bind(format!("mk-{machine}"))
            .execute(&pool)
            .await
            .unwrap();
        let sid = format!("{name}-{uid}");
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status, \
             adapter_id) VALUES ($1, $2, $2, $3, '/w', 'active', 'claude-code')",
        )
        .bind(&sid)
        .bind(machine)
        .bind(uid)
        .execute(&pool)
        .await
        .unwrap();
        Some((pool, sid))
    }

    async fn insert_message(pool: &PgPool, sid: &str, text: &str) -> i64 {
        sqlx::query_scalar(
            "INSERT INTO stream_events (session_id, event_type, payload) \
             VALUES ($1, 'message', jsonb_build_object('role', 'assistant', 'text', $2::text)) \
             RETURNING id",
        )
        .bind(sid)
        .bind(text)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn chunk_count(pool: &PgPool, sid: &str) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM search_chunks WHERE session_id = $1")
            .bind(sid)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn drain(walk: &mut Backfill, pool: &PgPool) -> usize {
        let mut steps = 0;
        while walk.step(pool).await.unwrap() > 0 {
            steps += 1;
            assert!(steps < 10_000, "backfill never caught up");
        }
        steps
    }

    #[tokio::test]
    async fn backfill_is_resumable_and_idempotent() {
        let name = "search_backfill_resumable";
        let Some((pool, sid)) = seeded(name).await else { return };
        let walk_name = format!("test-{sid}");
        let mut ids = Vec::new();
        for i in 0..5 {
            ids.push(insert_message(&pool, &sid, &format!("event number {i} with `code`")).await);
        }
        let long = format!("{} after the cut {}", "pad ".repeat(3000), "x ".repeat(10));
        ids.push(insert_message(&pool, &sid, &long).await);
        sqlx::query("INSERT INTO search_index_state (name, last_event_id) VALUES ($1, $2)")
            .bind(&walk_name)
            .bind(ids[0] - 1)
            .execute(&pool)
            .await
            .unwrap();

        let mut walk =
            Backfill { name: walk_name.clone(), batch: 2, horizon: Horizon::new(Duration::ZERO) };
        assert_eq!(walk.step(&pool).await.unwrap(), 2);
        let cursor: i64 =
            sqlx::query_scalar("SELECT last_event_id FROM search_index_state WHERE name = $1")
                .bind(&walk_name)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(
            (ids[0]..=ids[1]).contains(&cursor),
            "the cursor stops at the last event of the batch: {cursor}"
        );

        let mut resumed =
            Backfill { name: walk_name.clone(), batch: 2, horizon: Horizon::new(Duration::ZERO) };
        drain(&mut resumed, &pool).await;
        let total = chunk_count(&pool, &sid).await;
        assert!(total > 6, "the long event spans several chunks: {total}");

        sqlx::query("UPDATE search_index_state SET last_event_id = $2 WHERE name = $1")
            .bind(&walk_name)
            .bind(ids[0] - 1)
            .execute(&pool)
            .await
            .unwrap();
        drain(&mut resumed, &pool).await;
        assert_eq!(chunk_count(&pool, &sid).await, total, "a re-run adds nothing");

        let after_cut: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM search_chunks WHERE session_id = $1 AND body ILIKE '%after the cut%'",
        )
        .bind(&sid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(after_cut >= 1, "text past 8 KB is indexed");

        sqlx::query("DELETE FROM stream_events WHERE id = $1")
            .bind(ids[0])
            .execute(&pool)
            .await
            .unwrap();
        let gone: i64 =
            sqlx::query_scalar("SELECT count(*) FROM search_chunks WHERE event_id = $1")
                .bind(ids[0])
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(gone, 0, "chunks follow their event out");

        sqlx::query("DELETE FROM search_index_state WHERE name = $1")
            .bind(&walk_name)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM sessions WHERE id = $1").bind(&sid).execute(&pool).await.unwrap();
        assert_eq!(chunk_count(&pool, &sid).await, 0, "chunks follow their session out");
    }

    #[tokio::test]
    async fn unchunked_reads_the_cursor_and_live_from_rows() {
        let Some((pool, sid)) = seeded("search_unchunked_state").await else { return };
        let (walk, live) = (format!("walk-{sid}"), format!("live-{sid}"));
        let set = |name: String, id: i64| {
            let pool = pool.clone();
            async move {
                sqlx::query(
                    "INSERT INTO search_index_state (name, last_event_id) VALUES ($1, $2) \
                     ON CONFLICT (name) DO UPDATE SET last_event_id = EXCLUDED.last_event_id",
                )
                .bind(name)
                .bind(id)
                .execute(&pool)
                .await
                .unwrap();
            }
        };
        assert_eq!(
            unchunked_of(&pool, &walk, &live).await.unwrap(),
            Some(Unchunked { after: 0, before: i64::MAX })
        );
        set(walk.clone(), 100).await;
        set(live.clone(), 500).await;
        assert_eq!(
            unchunked_of(&pool, &walk, &live).await.unwrap(),
            Some(Unchunked { after: 100, before: 500 })
        );
        set(walk.clone(), 500).await;
        assert_eq!(unchunked_of(&pool, &walk, &live).await.unwrap(), None);
        sqlx::query("DELETE FROM search_index_state WHERE name IN ($1, $2)")
            .bind(&walk)
            .bind(&live)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM sessions WHERE id = $1").bind(&sid).execute(&pool).await.unwrap();
    }

    #[tokio::test]
    async fn reindex_replaces_rewritten_text() {
        let Some((pool, sid)) = seeded("search_reindex").await else { return };
        let id = insert_message(&pool, &sid, "token sk-live-secret here").await;
        let rows = vec![Chunked {
            event_id: id,
            session_id: sid.clone(),
            bodies: vec!["token sk-live-secret here".into()],
        }];
        insert_chunks(&pool, &rows).await.unwrap();
        sqlx::query("UPDATE stream_events SET payload = jsonb_set(payload, '{text}', '\"token [REDACTED] here\"') WHERE id = $1")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        reindex(&pool, &[id]).await.unwrap();
        let bodies: Vec<String> =
            sqlx::query_scalar("SELECT body FROM search_chunks WHERE event_id = $1")
                .bind(id)
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(bodies, ["token [REDACTED] here"]);
        sqlx::query("DELETE FROM sessions WHERE id = $1").bind(&sid).execute(&pool).await.unwrap();
    }
}
