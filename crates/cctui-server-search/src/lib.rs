//! Daemon events are chunked on ingest; every other row, history included, is
//! reached by the backfill, which `CCTUI_SEARCH_BACKFILL=0` pauses in place.

pub mod schema;
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
/// Only user and assistant messages and thinking are indexed.
#[must_use]
pub fn chunk_event(adapter_id: &str, event_type: &str, payload: Value) -> Vec<String> {
    cctui_server_kernel::normalize::for_client(adapter_id, event_type, payload)
        .filter(is_searchable)
        .map_or_else(Vec::new, |client| {
            let text = text::client_text(&client);
            text::chunks(&text).into_iter().map(str::to_owned).collect()
        })
}

fn is_searchable(client: &Value) -> bool {
    matches!(client.get("kind").and_then(Value::as_str), None | Some("thinking"))
        && matches!(
            cctui_server_kernel::normalize::client_category(client).0,
            "user" | "assistant" | "thinking"
        )
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

/// Insert freshly ingested events' chunks in the background; a failure only
/// delays them until the backfill reaches their ids.
pub fn index_new(pool: &PgPool, rows: Vec<Chunked>) {
    if rows.is_empty() {
        return;
    }
    let pool = pool.clone();
    tokio::spawn(async move {
        if let Some(first) = rows.iter().map(|e| e.event_id).min()
            && let Err(err) = record_live_from(&pool, first).await
        {
            tracing::warn!(%err, "recording the first inline-indexed event failed");
        }
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

/// Backfill settings, read from the environment.
///
/// `CCTUI_SEARCH_BACKFILL` (on unless `0`/`false`/`off`/`no`),
/// `CCTUI_SEARCH_BACKFILL_BATCH` (events per batch, default 50),
/// `CCTUI_SEARCH_BACKFILL_PAUSE_MS` (sleep between batches, default 250) and
/// `CCTUI_SEARCH_BACKFILL_BUDGET_MB` (chunk body bytes this process writes
/// before the walk stops, unlimited by default).
pub struct BackfillConfig {
    pub batch: i64,
    pub pause: Duration,
    pub budget: Option<u64>,
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
            budget: num("CCTUI_SEARCH_BACKFILL_BUDGET_MB").map(|mb| mb.saturating_mul(1 << 20)),
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

#[derive(Default)]
struct Written {
    batches: u64,
    chunks: u64,
    bytes: u64,
}

/// One resumable walk over `stream_events`, keyed by its `search_index_state`
/// name. It skips events that already have chunks.
pub struct Backfill {
    name: String,
    batch: i64,
    horizon: Horizon,
    written: Written,
    #[cfg(test)]
    session: Option<String>,
}

const PROGRESS_EVERY: u64 = 100;

const UNCHUNKED: &str = " AND NOT EXISTS (SELECT 1 FROM search_chunks k WHERE k.event_id = e.id)";

#[cfg(not(test))]
const SESSION_SCOPE: &str = "";
#[cfg(test)]
const SESSION_SCOPE: &str = " AND ($4::text IS NULL OR e.session_id = $4)";

impl Backfill {
    #[must_use]
    pub fn new(name: &str, batch: i64) -> Self {
        Self {
            name: name.to_owned(),
            batch,
            horizon: Horizon::new(COMMIT_LAG),
            written: Written::default(),
            #[cfg(test)]
            session: None,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn without_lag(name: &str, batch: i64) -> Self {
        Self { horizon: Horizon::new(Duration::ZERO), ..Self::new(name, batch) }
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
        let query = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "{SOURCE_SELECT} WHERE e.id > $1 AND e.id <= $2{UNCHUNKED}{SESSION_SCOPE} \
             ORDER BY e.id LIMIT $3"
        )))
        .bind(cursor)
        .bind(ceiling)
        .bind(self.batch);
        #[cfg(test)]
        let query = query.bind(self.session.as_deref());
        let rows: Vec<SourceRow> = query.fetch_all(&mut *tx).await?;
        let read = rows.len();
        let next = if usize::try_from(self.batch).is_ok_and(|b| read < b) {
            ceiling
        } else {
            rows.last().map_or(ceiling, |r| r.id)
        };
        let chunked = chunk_rows(rows);
        let chunks = insert_chunks(&mut *tx, &chunked).await?;
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
        self.note_batch(&chunked, chunks, next, max_id);
        Ok(read)
    }

    fn note_batch(&mut self, chunked: &[Chunked], chunks: u64, cursor: i64, max_id: i64) {
        let bytes = chunked.iter().flat_map(|c| &c.bodies).map(String::len).sum::<usize>();
        let w = &mut self.written;
        w.batches += 1;
        w.chunks += chunks;
        w.bytes = w.bytes.saturating_add(u64::try_from(bytes).unwrap_or(u64::MAX));
        if w.batches.is_multiple_of(PROGRESS_EVERY) {
            tracing::info!(
                walk = %self.name,
                cursor,
                max_id,
                batches = w.batches,
                chunks = w.chunks,
                bytes = w.bytes,
                "search backfill progress"
            );
        }
    }

    fn spent(&self, budget: Option<u64>) -> bool {
        budget.is_some_and(|b| self.written.bytes >= b)
    }
}

/// The production backfill loop: a batch, a pause, and a slow poll once it
/// has caught up so new rows from every insert path are reached too.
pub async fn task(pool: PgPool, config: BackfillConfig) {
    run(&pool, &mut Backfill::new(BACKFILL, config.batch), &config).await;
}

/// Walks until the budget is spent; a restart resumes from the cursor.
async fn run(pool: &PgPool, walk: &mut Backfill, config: &BackfillConfig) {
    let full = usize::try_from(config.batch).unwrap_or(usize::MAX);
    loop {
        let wait = match walk.step(pool).await {
            Ok(n) if n >= full => config.pause,
            Ok(_) => IDLE,
            Err(err) => {
                tracing::warn!(%err, "search backfill batch failed");
                ERROR_BACKOFF
            }
        };
        if walk.spent(config.budget) {
            let w = &walk.written;
            tracing::info!(
                batches = w.batches,
                chunks = w.chunks,
                bytes = w.bytes,
                "search backfill budget spent; restart to continue"
            );
            return;
        }
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
        assert_eq!((on.batch, on.pause, on.budget), (DEFAULT_BATCH, DEFAULT_PAUSE, None));
        for v in ["0", "false", "OFF", " no "] {
            let get = |k: &str| (k == "CCTUI_SEARCH_BACKFILL").then(|| v.to_owned());
            assert!(BackfillConfig::from_lookup(get).is_none(), "{v}");
        }
        let tuned = BackfillConfig::from_lookup(|k: &str| match k {
            "CCTUI_SEARCH_BACKFILL_BATCH" => Some("10".into()),
            "CCTUI_SEARCH_BACKFILL_PAUSE_MS" => Some("1000".into()),
            "CCTUI_SEARCH_BACKFILL_BUDGET_MB" => Some("3".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!((tuned.batch, tuned.pause), (10, Duration::from_secs(1)));
        assert_eq!(tuned.budget, Some(3 << 20));
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
    fn tool_calls_and_results_yield_no_chunks() {
        let blob = "iVBORw0KGgoAAAANSUhEUgAA".repeat(40);
        let image_result = json!({
            "kind": "tool_result",
            "tool_use_id": "t1",
            "content": [
                {"type": "text", "text": "captured"},
                {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": blob}},
            ],
        });
        let text_result = json!({"kind": "tool_result", "tool_use_id": "t2", "content": "total 0"});
        let call = json!({"tool": "Bash", "id": "t2", "input": {"command": "ls -la"}});
        let server_call = json!({"kind": "server_tool_use", "tool": "web_search", "id": "t3",
            "input": {"query": "rust"}});
        for payload in [image_result, text_result, call, server_call] {
            assert!(
                chunk_event("claude-code", "tool_use", payload.clone()).is_empty(),
                "{payload}"
            );
        }
        let codex_call = json!({"type": "response_item", "payload": {"type": "function_call",
            "name": "shell", "call_id": "c1", "arguments": "{\"command\":[\"ls\"]}"}});
        let codex_out = json!({"type": "response_item", "payload": {"type": "function_call_output",
            "call_id": "c1", "output": "file.txt"}});
        for payload in [codex_call, codex_out] {
            assert!(chunk_event("codex", "tool_use", payload.clone()).is_empty(), "{payload}");
        }
    }

    #[test]
    fn markers_annotations_and_summaries_yield_no_chunks() {
        let message = |p: Value| chunk_event("claude-code", "message", p);
        for payload in [
            json!({"role": "system_marker", "marker": "ai-title", "text": "title: deploy"}),
            json!({"role": "system_marker", "marker": "queue-operation", "operation": "enqueue",
                "text": "enqueue: ship it", "queue_text": "ship it"}),
            json!({"role": "turn_annotation", "annotation": "turn_duration", "text": "turn_duration:1234"}),
            json!({"role": "summary", "status_category": "done", "status_detail": "shipped the fix"}),
            json!({"role": "compact_summary", "text": "summary of the conversation"}),
            json!({"role": "assistant_redacted_thinking", "text": "[redacted]"}),
            json!({"role": "assistant_attachment", "text": "attached file"}),
            json!({"role": "assistant_voice", "text": "hello there", "message_id": "n1"}),
            json!({"role": "user", "text": "<task-notification/>", "meta": true}),
        ] {
            assert!(message(payload.clone()).is_empty(), "{payload}");
        }
        let ended = json!({"reason": {"kind": "completed", "detail": "all done"}});
        assert!(chunk_event("claude-code", "session_ended", ended).is_empty());
    }

    #[test]
    fn user_assistant_and_thinking_messages_are_chunked() {
        let message = |p: Value| chunk_event("claude-code", "message", p);
        assert_eq!(message(json!({"role": "user", "text": "ship the **fix**"})), ["ship the fix"]);
        assert_eq!(
            message(json!({"role": "assistant", "text": "on it", "message_id": "m1"})),
            ["on it"]
        );
        assert_eq!(
            message(json!({"role": "assistant_thinking", "text": "pondering", "message_id": "m1"})),
            ["pondering"]
        );
        let codex = |p: Value| chunk_event("codex", "message", p);
        assert_eq!(codex(json!({"type": "agentMessage", "text": "done"})), ["done"]);
        assert_eq!(
            codex(json!({"type": "reasoning", "summary": ["weighing it"]})),
            ["weighing it"]
        );
        assert_eq!(
            codex(
                json!({"type": "event_msg", "payload": {"type": "user_message", "message": "go"}})
            ),
            ["go"]
        );
    }

    async fn seeded(name: &str) -> Option<(PgPool, String)> {
        let url = cctui_server_kernel::test_support::db_url(name)?;
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

    fn scoped_walk(name: &str, sid: &str) -> Backfill {
        Backfill {
            name: name.to_owned(),
            batch: 2,
            horizon: Horizon::new(Duration::ZERO),
            written: Written::default(),
            session: Some(sid.to_owned()),
        }
    }

    async fn start_walk_before(pool: &PgPool, walk_name: &str, first: i64) {
        sqlx::query("INSERT INTO search_index_state (name, last_event_id) VALUES ($1, $2)")
            .bind(walk_name)
            .bind(first - 1)
            .execute(pool)
            .await
            .unwrap();
    }

    async fn cursor_of(pool: &PgPool, walk_name: &str) -> i64 {
        sqlx::query_scalar("SELECT last_event_id FROM search_index_state WHERE name = $1")
            .bind(walk_name)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn event_chunks(pool: &PgPool, id: i64) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM search_chunks WHERE event_id = $1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn drop_walk(pool: &PgPool, walk_name: &str, sid: &str) {
        sqlx::query("DELETE FROM search_index_state WHERE name = $1")
            .bind(walk_name)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM sessions WHERE id = $1").bind(sid).execute(pool).await.unwrap();
    }

    #[tokio::test]
    async fn the_walk_skips_events_that_already_have_chunks() {
        let Some((pool, sid)) = seeded("search_backfill_skip_chunked").await else { return };
        let walk_name = format!("test-{sid}");
        let first = insert_message(&pool, &sid, "a plain event").await;
        let long = format!("{} tail", "pre chunked ".repeat(1000));
        let pre = insert_message(&pool, &sid, &long).await;
        insert_message(&pool, &sid, "another plain event").await;
        let one =
            vec![Chunked { event_id: pre, session_id: sid.clone(), bodies: vec!["x".into()] }];
        insert_chunks(&pool, &one).await.unwrap();
        start_walk_before(&pool, &walk_name, first).await;

        let mut walk = scoped_walk(&walk_name, &sid);
        drain(&mut walk, &pool).await;
        assert_eq!(event_chunks(&pool, pre).await, 1, "the pre-chunked event is not re-chunked");
        assert_eq!(event_chunks(&pool, first).await, 1);
        let total = chunk_count(&pool, &sid).await;
        assert_eq!(total, 3);
        assert_eq!(walk.written.chunks, 2, "only the two unchunked events are written");

        sqlx::query("UPDATE search_index_state SET last_event_id = $2 WHERE name = $1")
            .bind(&walk_name)
            .bind(first - 1)
            .execute(&pool)
            .await
            .unwrap();
        let mut rerun = scoped_walk(&walk_name, &sid);
        drain(&mut rerun, &pool).await;
        assert_eq!(chunk_count(&pool, &sid).await, total, "a re-run adds nothing");
        assert_eq!((rerun.written.chunks, rerun.written.bytes), (0, 0), "and reads nothing");
        assert!(cursor_of(&pool, &walk_name).await > first, "the cursor still moves on");

        drop_walk(&pool, &walk_name, &sid).await;
    }

    #[tokio::test]
    async fn the_walk_stops_once_its_budget_is_spent_and_a_restart_resumes() {
        let Some((pool, sid)) = seeded("search_backfill_budget").await else { return };
        let walk_name = format!("test-{sid}");
        let mut ids = Vec::new();
        for i in 0..6 {
            ids.push(insert_message(&pool, &sid, &format!("budgeted event {i}")).await);
        }
        start_walk_before(&pool, &walk_name, ids[0]).await;
        let config = BackfillConfig { batch: 2, pause: Duration::ZERO, budget: Some(1) };

        let mut walk = scoped_walk(&walk_name, &sid);
        let stopped = tokio::time::timeout(Duration::from_secs(30), run(&pool, &mut walk, &config));
        stopped.await.expect("the walk stops on its budget");
        assert_eq!(walk.written.batches, 1, "one batch spends a one-byte budget");
        assert_eq!(cursor_of(&pool, &walk_name).await, ids[1]);
        assert_eq!(chunk_count(&pool, &sid).await, 2);

        let mut restarted = scoped_walk(&walk_name, &sid);
        drain(&mut restarted, &pool).await;
        assert_eq!(chunk_count(&pool, &sid).await, 6, "a restart picks up at the cursor");
        assert_eq!(restarted.written.chunks, 4);

        drop_walk(&pool, &walk_name, &sid).await;
    }

    #[tokio::test]
    async fn the_progress_line_does_not_break_its_batch() {
        let Some((pool, sid)) = seeded("search_backfill_progress").await else { return };
        let walk_name = format!("test-{sid}");
        let first = insert_message(&pool, &sid, "logged event one").await;
        insert_message(&pool, &sid, "logged event two").await;
        start_walk_before(&pool, &walk_name, first).await;

        let mut walk = scoped_walk(&walk_name, &sid);
        walk.written.batches = PROGRESS_EVERY - 1;
        assert_eq!(walk.step(&pool).await.unwrap(), 2);
        assert_eq!(walk.written.batches, PROGRESS_EVERY);
        assert_eq!(walk.written.chunks, 2);
        assert_eq!(chunk_count(&pool, &sid).await, 2, "the logged batch is committed");

        drop_walk(&pool, &walk_name, &sid).await;
    }

    #[tokio::test]
    async fn a_session_scoped_backfill_leaves_other_sessions_alone() {
        let Some((pool, mine)) = seeded("search_backfill_scope_mine").await else { return };
        let Some((other_pool, other)) = seeded("search_backfill_scope_other").await else {
            return;
        };
        let first = insert_message(&pool, &mine, "mine before").await;
        insert_message(&pool, &other, "someone else's event").await;
        insert_message(&pool, &mine, "mine after").await;
        let walk_name = format!("test-{mine}");
        sqlx::query("INSERT INTO search_index_state (name, last_event_id) VALUES ($1, $2)")
            .bind(&walk_name)
            .bind(first - 1)
            .execute(&pool)
            .await
            .unwrap();

        drain(&mut scoped_walk(&walk_name, &mine), &pool).await;
        assert!(chunk_count(&pool, &mine).await >= 2, "both of its own events are indexed");
        assert_eq!(chunk_count(&pool, &other).await, 0, "the walk only reads its own session");

        sqlx::query("DELETE FROM search_index_state WHERE name = $1")
            .bind(&walk_name)
            .execute(&pool)
            .await
            .unwrap();
        for (p, sid) in [(&pool, &mine), (&other_pool, &other)] {
            sqlx::query("DELETE FROM sessions WHERE id = $1").bind(sid).execute(p).await.unwrap();
        }
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

        let mut walk = scoped_walk(&walk_name, &sid);
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

        let mut resumed = scoped_walk(&walk_name, &sid);
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
