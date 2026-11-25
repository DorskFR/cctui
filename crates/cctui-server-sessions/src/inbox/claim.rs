use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::outbox::{CLAIM_LEASE_SECS, Retry, retry_after_failure};

pub const MAX_BATCH: i64 = 20;

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct ClaimedLane {
    pub id: Uuid,
    pub user_id: Uuid,
    pub key: String,
    pub preset_id: Option<Uuid>,
    pub session_id: Option<String>,
    pub session_status: Option<String>,
    pub spawn_command_id: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct BatchEvent {
    pub seq: i64,
    pub user_id: Uuid,
    pub source: String,
    pub body: String,
    pub sealed_body: Option<String>,
    pub parent_session_id: Option<String>,
    pub attempts: i32,
    pub batch_id: Uuid,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LaneFilter {
    pub lane: Option<Uuid>,
    /// A daemon that just connected: its queued spawns are reachable before
    /// its first heartbeat lands.
    pub machine: Option<Uuid>,
}

/// The outer lease predicate is what keeps two replicas from both winning a lane.
const CLAIM_LANES_SQL: &str = "\
    WITH due AS ( \
        SELECT l.id FROM inbox_lanes l \
        LEFT JOIN sessions s ON s.id = l.session_id \
        LEFT JOIN machines m ON m.id = s.machine_uuid \
        WHERE NOT l.paused \
          AND ($2::uuid IS NULL OR l.id = $2) \
          AND ($3::uuid IS NULL OR s.machine_uuid = $3) \
          AND (l.claimed_until IS NULL OR l.claimed_until < now()) \
          AND EXISTS (SELECT 1 FROM inbox_events e \
                      WHERE e.lane_id = l.id AND e.status = 'pending' \
                        AND e.not_before <= now() AND e.next_attempt_at <= now()) \
          AND COALESCE(s.tempo, '') NOT IN ('blocked', 'active') \
          AND COALESCE(s.agent_state, '') NOT IN ('working', 'running') \
          AND s.soft_limit_reason IS NULL \
          AND (s.status IS DISTINCT FROM 'queued' OR $3::uuid IS NOT NULL \
               OR m.last_seen_at > now() - $4::bigint * interval '1 second') \
        ORDER BY l.claimed_until NULLS FIRST \
        LIMIT $1 \
        FOR UPDATE OF l SKIP LOCKED) \
    UPDATE inbox_lanes l SET claimed_until = now() + $5::bigint * interval '1 second' \
    FROM due \
    WHERE l.id = due.id AND (l.claimed_until IS NULL OR l.claimed_until < now()) \
    RETURNING l.id, l.user_id, l.key, l.preset_id, l.session_id, \
              (SELECT status FROM sessions WHERE id = l.session_id) AS session_status, \
              l.spawn_command_id";

pub async fn claim_lanes(
    pool: &PgPool,
    limit: i64,
    filter: LaneFilter,
) -> sqlx::Result<Vec<ClaimedLane>> {
    sqlx::query_as(CLAIM_LANES_SQL)
        .bind(limit)
        .bind(filter.lane)
        .bind(filter.machine)
        .bind(cctui_server_kernel::dispatch_types::ONLINE_SECS)
        .bind(CLAIM_LEASE_SECS)
        .fetch_all(pool)
        .await
}

/// A batch stranded by a crashed replica is taken again whole, under its old
/// `batch_id`, so the turn dedupe absorbs a redelivery. Otherwise every due
/// event joins a fresh batch.
const CLAIM_BATCH_SQL: &str = "\
    WITH stranded AS ( \
        SELECT batch_id FROM inbox_events \
        WHERE lane_id = $1 AND status = 'pending' AND batch_id IS NOT NULL \
          AND not_before <= now() AND next_attempt_at <= now() \
        ORDER BY seq LIMIT 1), \
    pick AS ( \
        SELECT id FROM inbox_events \
        WHERE lane_id = $1 AND status = 'pending' \
          AND not_before <= now() AND next_attempt_at <= now() \
          AND (expires_at IS NULL OR expires_at > now()) \
          AND batch_id IS NOT DISTINCT FROM (SELECT batch_id FROM stranded) \
        ORDER BY seq LIMIT $2 \
        FOR UPDATE SKIP LOCKED) \
    UPDATE inbox_events e \
    SET status = 'claimed', batch_id = COALESCE(e.batch_id, $3), \
        next_attempt_at = now() + $4::bigint * interval '1 second' \
    FROM pick WHERE e.id = pick.id \
    RETURNING e.seq, e.user_id, e.source, e.body, e.sealed_body, e.parent_session_id, \
              e.attempts, e.batch_id, e.created_at";

pub async fn claim_batch(pool: &PgPool, lane_id: Uuid) -> sqlx::Result<Vec<BatchEvent>> {
    let mut events: Vec<BatchEvent> = sqlx::query_as(CLAIM_BATCH_SQL)
        .bind(lane_id)
        .bind(MAX_BATCH)
        .bind(Uuid::new_v4())
        .bind(CLAIM_LEASE_SECS)
        .fetch_all(pool)
        .await?;
    events.sort_by_key(|e| e.seq);
    Ok(events)
}

pub async fn release_lane(pool: &PgPool, lane_id: Uuid) -> sqlx::Result<()> {
    sqlx::query("UPDATE inbox_lanes SET claimed_until = NULL WHERE id = $1")
        .bind(lane_id)
        .execute(pool)
        .await
        .map(|_| ())
}

pub async fn mark_delivered(
    pool: &PgPool,
    batch_id: Uuid,
    session_id: &str,
    delivered_as: &str,
) -> sqlx::Result<u64> {
    sqlx::query(
        "UPDATE inbox_events \
         SET status = 'delivered', delivered_at = now(), delivered_session_id = $2, \
             delivered_as = $3, last_error = NULL \
         WHERE batch_id = $1 AND status = 'claimed'",
    )
    .bind(batch_id)
    .bind(session_id)
    .bind(delivered_as)
    .execute(pool)
    .await
    .map(|done| done.rows_affected())
}

pub async fn defer(pool: &PgPool, batch_id: Uuid) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE inbox_events SET status = 'pending', next_attempt_at = now() \
         WHERE batch_id = $1 AND status = 'claimed'",
    )
    .bind(batch_id)
    .execute(pool)
    .await
    .map(|_| ())
}

pub async fn mark_dead(
    pool: &PgPool,
    batch_id: Uuid,
    attempts: i32,
    reason: &str,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE inbox_events SET status = 'dead', attempts = $2, last_error = $3 \
         WHERE batch_id = $1 AND status = 'claimed'",
    )
    .bind(batch_id)
    .bind(attempts)
    .bind(reason)
    .execute(pool)
    .await
    .map(|_| ())
}

/// A retried batch is dissolved so newer events can join the next one.
pub async fn record_failure(
    pool: &PgPool,
    batch_id: Uuid,
    attempts: i32,
    err: &str,
) -> sqlx::Result<Retry> {
    retry_batch(pool, batch_id, attempts, err, false).await
}

/// A batch whose delivery may have landed keeps its `batch_id`, so the next
/// attempt can look the earlier one up before sending again.
pub async fn record_ambiguous_failure(
    pool: &PgPool,
    batch_id: Uuid,
    attempts: i32,
    err: &str,
) -> sqlx::Result<Retry> {
    retry_batch(pool, batch_id, attempts, err, true).await
}

async fn retry_batch(
    pool: &PgPool,
    batch_id: Uuid,
    attempts: i32,
    err: &str,
    keep_batch: bool,
) -> sqlx::Result<Retry> {
    let retry = retry_after_failure(attempts);
    match retry {
        Retry::Dead { attempts } => mark_dead(pool, batch_id, attempts, err).await?,
        Retry::After { attempts, secs } => {
            sqlx::query(
                "UPDATE inbox_events \
                 SET status = 'pending', batch_id = CASE WHEN $5 THEN batch_id END, \
                     attempts = $2, last_error = $3, \
                     next_attempt_at = now() + $4::bigint * interval '1 second' \
                 WHERE batch_id = $1 AND status = 'claimed'",
            )
            .bind(batch_id)
            .bind(attempts)
            .bind(err)
            .bind(secs)
            .bind(keep_batch)
            .execute(pool)
            .await?;
        }
    }
    Ok(retry)
}

pub async fn reclaim_stranded(pool: &PgPool) -> sqlx::Result<u64> {
    sqlx::query(
        "UPDATE inbox_events SET status = 'pending' \
         WHERE status = 'claimed' AND next_attempt_at < now()",
    )
    .execute(pool)
    .await
    .map(|done| done.rows_affected())
}

pub async fn cancel_expired(pool: &PgPool) -> sqlx::Result<u64> {
    sqlx::query(
        "UPDATE inbox_events SET status = 'cancelled', last_error = 'expired before delivery' \
         WHERE status = 'pending' AND expires_at <= now()",
    )
    .execute(pool)
    .await
    .map(|done| done.rows_affected())
}
