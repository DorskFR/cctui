use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use super::{RETENTION_DAYS, Source, Status, session_lane_key};

const LIST_LIMIT: i64 = 200;
const PRUNE_BATCH: i64 = 1000;

pub struct NewEvent<'a> {
    pub lane_id: Uuid,
    pub user_id: Uuid,
    pub source: Source,
    pub source_id: Option<&'a str>,
    pub body: &'a str,
    pub payload: Option<&'a serde_json::Value>,
    pub not_before: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Inserted {
    pub id: Uuid,
    pub deduplicated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cancel {
    Cancelled,
    NotPending(String),
    NotFound,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Event {
    pub id: Uuid,
    pub lane_id: Uuid,
    pub lane_key: String,
    pub source: String,
    pub source_id: Option<String>,
    pub body: String,
    pub payload: Option<serde_json::Value>,
    pub status: String,
    pub not_before: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub attempts: i32,
    pub last_error: Option<String>,
    pub batch_id: Option<Uuid>,
    pub delivered_session_id: Option<String>,
    pub delivered_as: Option<String>,
    pub created_at: DateTime<Utc>,
    pub delivered_at: Option<DateTime<Utc>>,
}

macro_rules! select_events {
    () => {
        "SELECT e.id, e.lane_id, l.key AS lane_key, e.source, e.source_id, e.body, \
                e.payload - 'raw' AS payload, \
                e.status, e.not_before, e.expires_at, e.attempts, e.last_error, e.batch_id, \
                e.delivered_session_id, e.delivered_as, e.created_at, e.delivered_at \
         FROM inbox_events e JOIN inbox_lanes l ON l.id = e.lane_id "
    };
}

/// `None` when the session does not exist; `Some(None)` when it has no owner.
pub async fn session_lane_owner(
    pool: &PgPool,
    session_id: &str,
) -> sqlx::Result<Option<Option<Uuid>>> {
    sqlx::query_scalar(
        "SELECT COALESCE(m.user_id, s.user_id) \
         FROM sessions s LEFT JOIN machines m ON m.id = s.machine_uuid WHERE s.id = $1",
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await
}

pub async fn session_lane(pool: &PgPool, user_id: Uuid, session_id: &str) -> sqlx::Result<Uuid> {
    sqlx::query_scalar(
        "INSERT INTO inbox_lanes (user_id, key, session_id) VALUES ($1, $2, $3) \
         ON CONFLICT (user_id, key) DO UPDATE \
         SET session_id = COALESCE(inbox_lanes.session_id, EXCLUDED.session_id) \
         RETURNING id",
    )
    .bind(user_id)
    .bind(session_lane_key(session_id))
    .bind(session_id)
    .fetch_one(pool)
    .await
}

pub async fn insert_event(pool: &PgPool, event: &NewEvent<'_>) -> sqlx::Result<Inserted> {
    let inserted: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO inbox_events \
           (lane_id, user_id, source, source_id, body, payload, not_before, next_attempt_at, \
            expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, COALESCE($7, now()), COALESCE($7, now()), $8) \
         ON CONFLICT (lane_id, source, source_id) DO NOTHING \
         RETURNING id",
    )
    .bind(event.lane_id)
    .bind(event.user_id)
    .bind(event.source.as_str())
    .bind(event.source_id)
    .bind(event.body)
    .bind(event.payload)
    .bind(event.not_before)
    .bind(event.expires_at)
    .fetch_optional(pool)
    .await?;
    if let Some(id) = inserted {
        return Ok(Inserted { id, deduplicated: false });
    }
    let id = sqlx::query_scalar(
        "SELECT id FROM inbox_events WHERE lane_id = $1 AND source = $2 AND source_id = $3",
    )
    .bind(event.lane_id)
    .bind(event.source.as_str())
    .bind(event.source_id)
    .fetch_one(pool)
    .await?;
    Ok(Inserted { id, deduplicated: true })
}

pub const MAX_HOPS: i16 = 8;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Lineage<'a> {
    pub hops: i16,
    pub parent_session_id: Option<&'a str>,
}

/// `Ok(None)`: refused, the event is past [`MAX_HOPS`].
pub async fn insert_traced(
    pool: &PgPool,
    event: &NewEvent<'_>,
    lineage: Lineage<'_>,
) -> sqlx::Result<Option<Inserted>> {
    if !(0..=MAX_HOPS).contains(&lineage.hops) {
        return Ok(None);
    }
    let inserted: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO inbox_events \
           (lane_id, user_id, source, source_id, body, payload, not_before, next_attempt_at, \
            expires_at, hops, parent_session_id) \
         VALUES ($1, $2, $3, $4, $5, $6, COALESCE($7, now()), COALESCE($7, now()), $8, $9, $10) \
         ON CONFLICT (lane_id, source, source_id) DO NOTHING \
         RETURNING id",
    )
    .bind(event.lane_id)
    .bind(event.user_id)
    .bind(event.source.as_str())
    .bind(event.source_id)
    .bind(event.body)
    .bind(event.payload)
    .bind(event.not_before)
    .bind(event.expires_at)
    .bind(lineage.hops)
    .bind(lineage.parent_session_id)
    .fetch_optional(pool)
    .await?;
    if let Some(id) = inserted {
        return Ok(Some(Inserted { id, deduplicated: false }));
    }
    let id = sqlx::query_scalar(
        "SELECT id FROM inbox_events WHERE lane_id = $1 AND source = $2 AND source_id = $3",
    )
    .bind(event.lane_id)
    .bind(event.source.as_str())
    .bind(event.source_id)
    .fetch_one(pool)
    .await?;
    Ok(Some(Inserted { id, deduplicated: true }))
}

/// `owner` is [`crate::auth::AuthContext::owner_filter`]; `lane` is a lane id or key.
pub async fn list(
    pool: &PgPool,
    owner: Option<Uuid>,
    lane: Option<&str>,
    status: Option<Status>,
) -> sqlx::Result<Vec<Event>> {
    sqlx::query_as(concat!(
        select_events!(),
        "WHERE ($1::uuid IS NULL OR e.user_id = $1) \
           AND ($2::text IS NULL OR l.key = $2 OR l.id::text = $2) \
           AND ($3::text IS NULL OR e.status = $3) \
         ORDER BY e.seq DESC LIMIT $4"
    ))
    .bind(owner)
    .bind(lane)
    .bind(status.map(Status::as_str))
    .bind(LIST_LIMIT)
    .fetch_all(pool)
    .await
}

pub async fn get(pool: &PgPool, owner: Option<Uuid>, id: Uuid) -> sqlx::Result<Option<Event>> {
    sqlx::query_as(concat!(
        select_events!(),
        "WHERE e.id = $1 AND ($2::uuid IS NULL OR e.user_id = $2)"
    ))
    .bind(id)
    .bind(owner)
    .fetch_optional(pool)
    .await
}

pub async fn cancel(pool: &PgPool, owner: Option<Uuid>, id: Uuid) -> sqlx::Result<Cancel> {
    let cancelled: Option<Uuid> = sqlx::query_scalar(
        "UPDATE inbox_events SET status = 'cancelled' \
         WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2) AND status = 'pending' \
         RETURNING id",
    )
    .bind(id)
    .bind(owner)
    .fetch_optional(pool)
    .await?;
    if cancelled.is_some() {
        return Ok(Cancel::Cancelled);
    }
    let status: Option<String> = sqlx::query_scalar(
        "SELECT status FROM inbox_events WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)",
    )
    .bind(id)
    .bind(owner)
    .fetch_optional(pool)
    .await?;
    Ok(status.map_or(Cancel::NotFound, Cancel::NotPending))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Retry {
    Retried { lane_id: Uuid },
    NotDead(String),
    NotFound,
}

pub async fn retry(pool: &PgPool, owner: Option<Uuid>, id: Uuid) -> sqlx::Result<Retry> {
    let lane_id: Option<Uuid> = sqlx::query_scalar(
        "UPDATE inbox_events \
         SET status = 'pending', attempts = 0, next_attempt_at = now(), last_error = NULL, \
             batch_id = NULL \
         WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2) AND status = 'dead' \
         RETURNING lane_id",
    )
    .bind(id)
    .bind(owner)
    .fetch_optional(pool)
    .await?;
    if let Some(lane_id) = lane_id {
        return Ok(Retry::Retried { lane_id });
    }
    let status: Option<String> = sqlx::query_scalar(
        "SELECT status FROM inbox_events WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)",
    )
    .bind(id)
    .bind(owner)
    .fetch_optional(pool)
    .await?;
    Ok(status.map_or(Retry::NotFound, Retry::NotDead))
}

/// Retention doubles as the source-id dedup window.
pub async fn prune_delivered(pool: &PgPool) -> sqlx::Result<u64> {
    let done = sqlx::query(
        "DELETE FROM inbox_events WHERE id IN ( \
             SELECT id FROM inbox_events \
             WHERE status = 'delivered' AND delivered_at < now() - make_interval(days => $1) \
             LIMIT $2)",
    )
    .bind(RETENTION_DAYS)
    .bind(PRUNE_BATCH)
    .execute(pool)
    .await?;
    Ok(done.rows_affected())
}

pub async fn sweep(pool: &PgPool) {
    match prune_delivered(pool).await {
        Ok(0) => {}
        Ok(n) => tracing::debug!(pruned = n, "inbox: pruned delivered events"),
        Err(e) => tracing::warn!("inbox prune failed: {e}"),
    }
}
