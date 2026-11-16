use chrono::{Duration, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use super::Source;
use super::claim::BatchEvent;
use super::deliver::Outcome;
use super::store::{self, NewEvent};
use crate::auth::AuthContext;
use crate::state::AppState;

pub const EXPIRY_HOURS: i64 = 24;
pub const EXPIRED: &str = "the machine's daemon did not come back within 24 hours";
const CANCELLED: &str = "cancelled by the user";
const EVENT_BODY: &str = "launch the queued spawn once the machine's daemon is back";

pub async fn enqueue(
    pool: &PgPool,
    owner: Uuid,
    actor: Uuid,
    session_id: &str,
) -> sqlx::Result<store::Inserted> {
    let lane_id = store::session_lane(pool, owner, session_id).await?;
    let payload = serde_json::json!({ "kind": "queued_spawn" });
    store::insert_event(
        pool,
        &NewEvent {
            lane_id,
            user_id: actor,
            source: Source::Human,
            source_id: Some(session_id),
            body: EVENT_BODY,
            payload: Some(&payload),
            not_before: None,
            expires_at: Some(Utc::now() + Duration::hours(EXPIRY_HOURS)),
        },
    )
    .await
}

/// Launch as whoever queued the spawn, with the scopes they hold now.
pub async fn launch(state: &AppState, session_id: &str, events: &[BatchEvent]) -> Outcome {
    let Some(actor) = events.first().map(|e| e.user_id) else { return Outcome::Skip };
    let scopes = match crate::store::acls::user_ceiling(&state.pool, actor).await {
        Ok(scopes) => scopes,
        Err(e) => return Outcome::Retry(e.to_string()),
    };
    let ctx = AuthContext { user_id: actor, key_id: Uuid::nil(), machine_id: None, scopes };
    match crate::routes::spawn::launch_queued(state, &ctx, session_id).await {
        Ok(launched) => Outcome::Delivered {
            session_id: launched
                .session_id
                .map_or_else(|| launched.command_id.to_string(), |id| id.to_string()),
            delivered_as: "spawn",
        },
        Err(failure) if failure.queue.is_some() => Outcome::Retry(failure.message().to_owned()),
        Err(failure) => Outcome::Dead(failure.message().to_owned()),
    }
}

pub async fn fail(pool: &PgPool, session_id: &str, reason: &str) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE sessions SET status = 'failed', ended_at = now(), end_reason = 'spawn_failed', \
             end_detail = $2 \
         WHERE id = $1 AND status = 'queued'",
    )
    .bind(session_id)
    .bind(reason)
    .execute(pool)
    .await
    .map(|_| ())
}

async fn cancel_lanes(pool: &PgPool, session_ids: &[String], reason: &str) -> sqlx::Result<u64> {
    sqlx::query(
        "UPDATE inbox_events SET status = 'cancelled', last_error = $2 \
         WHERE status = 'pending' \
           AND lane_id IN (SELECT id FROM inbox_lanes WHERE session_id = ANY($1))",
    )
    .bind(session_ids)
    .bind(reason)
    .execute(pool)
    .await
    .map(|done| done.rows_affected())
}

pub async fn expire(pool: &PgPool) -> sqlx::Result<Vec<String>> {
    let expired: Vec<String> = sqlx::query_scalar(
        "UPDATE sessions SET status = 'failed', ended_at = now(), end_reason = 'spawn_failed', \
             end_detail = $1 \
         WHERE status = 'queued' AND registered_at < now() - $2::bigint * interval '1 hour' \
         RETURNING id",
    )
    .bind(EXPIRED)
    .bind(EXPIRY_HOURS)
    .fetch_all(pool)
    .await?;
    if !expired.is_empty() {
        cancel_lanes(pool, &expired, EXPIRED).await?;
    }
    Ok(expired)
}

/// Cancel a queued spawn's pending events; a no-op for any other session.
pub async fn cancel(pool: &PgPool, session_id: &str) -> sqlx::Result<u64> {
    let queued: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM sessions WHERE id = $1 AND status = 'queued')",
    )
    .bind(session_id)
    .fetch_one(pool)
    .await?;
    if !queued {
        return Ok(0);
    }
    cancel_lanes(pool, &[session_id.to_owned()], CANCELLED).await
}
