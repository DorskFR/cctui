use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use super::claim::ClaimedLane;
use crate::context::SessionsContext;
use cctui_server_kernel::notify_types::{self as notify, NewNotification};

pub const EVENTS_PER_HOUR: i64 = 60;
pub const STUCK_AFTER_SECS: i64 = 30 * 60;
pub const CEILING: &str = "events_per_hour";
pub const MANUAL: &str = "manual";
const STUCK_BATCH: i64 = 50;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct Lane {
    pub id: Uuid,
    pub key: String,
    pub preset_id: Option<Uuid>,
    pub session_id: Option<String>,
    pub paused: bool,
    pub paused_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
struct Flagged {
    id: Uuid,
    user_id: Uuid,
    key: String,
    session_id: Option<String>,
}

fn lane_notice(lane: &Flagged, collapse: &str, title: String, body: String) -> NewNotification {
    let mut n = NewNotification::new(lane.user_id, notify::kind::INBOX_STUCK, title);
    n.severity = "high";
    n.session_id.clone_from(&lane.session_id);
    n.subject_kind = Some("inbox_lane".into());
    n.subject_id = Some(lane.id.to_string());
    n.collapse_key = Some(format!("{collapse}:{}", lane.id));
    n.body = Some(body);
    n
}

/// The window restarts at resume, so a resumed lane is not paused again by
/// the burst that paused it.
const TRIP_CEILING_SQL: &str = "\
    UPDATE inbox_lanes l SET paused = true, paused_reason = $2 \
    WHERE l.id = $1 AND NOT l.paused \
      AND (SELECT count(*) FROM inbox_events e \
           WHERE e.lane_id = l.id \
             AND e.created_at > GREATEST(now() - interval '1 hour', \
                                         COALESCE(l.ceiling_from, '-infinity'::timestamptz))) > $3 \
    RETURNING l.id, l.user_id, l.key, l.session_id";

/// `Some` only for the call that paused the lane.
pub async fn trip_ceiling(pool: &PgPool, lane_id: Uuid) -> sqlx::Result<Option<NewNotification>> {
    let tripped: Option<Flagged> = sqlx::query_as(TRIP_CEILING_SQL)
        .bind(lane_id)
        .bind(CEILING)
        .bind(EVENTS_PER_HOUR)
        .fetch_optional(pool)
        .await?;
    Ok(tripped.map(|lane| {
        tracing::warn!(lane = %lane.key, "inbox lane paused: over {EVENTS_PER_HOUR} events/hour");
        lane_notice(
            &lane,
            "inbox_ceiling",
            format!("Inbox lane paused: {}", lane.key),
            format!(
                "More than {EVENTS_PER_HOUR} events reached this lane within an hour, which \
                 looks like a loop. Its events wait until you resume it \
                 (POST /api/v1/inbox/lanes/{}/resume).",
                lane.id
            ),
        )
    }))
}

#[must_use]
pub fn dead_lettered(lane: &ClaimedLane, batch_id: Uuid, reason: &str) -> NewNotification {
    let flagged = Flagged {
        id: lane.id,
        user_id: lane.user_id,
        key: lane.key.clone(),
        session_id: lane.session_id.clone(),
    };
    lane_notice(
        &flagged,
        "inbox_dead",
        format!("Inbox events not delivered: {}", lane.key),
        format!("Batch {batch_id} was dead-lettered: {reason}"),
    )
}

const FLAG_STUCK_SQL: &str = "\
    UPDATE inbox_lanes l SET stuck_alerted_at = now() \
    WHERE l.id IN ( \
        SELECT s.id FROM inbox_lanes s \
        WHERE s.stuck_alerted_at IS NULL AND NOT s.paused \
          AND ($3::uuid IS NULL OR s.id = $3) \
          AND EXISTS (SELECT 1 FROM inbox_events e \
                      WHERE e.lane_id = s.id AND e.status = 'pending' \
                        AND e.not_before < now() - $1::bigint * interval '1 second') \
        LIMIT $2 \
        FOR UPDATE SKIP LOCKED) \
    RETURNING l.id, l.user_id, l.key, l.session_id";

/// One notice per stuck spell: the flag clears once the lane drains.
pub async fn flag_stuck(pool: &PgPool, only: Option<Uuid>) -> sqlx::Result<Vec<NewNotification>> {
    sqlx::query(
        "UPDATE inbox_lanes l SET stuck_alerted_at = NULL \
         WHERE l.stuck_alerted_at IS NOT NULL AND ($2::uuid IS NULL OR l.id = $2) \
           AND NOT EXISTS (SELECT 1 FROM inbox_events e \
                           WHERE e.lane_id = l.id AND e.status = 'pending' \
                             AND e.not_before < now() - $1::bigint * interval '1 second')",
    )
    .bind(STUCK_AFTER_SECS)
    .bind(only)
    .execute(pool)
    .await?;
    let stuck: Vec<Flagged> = sqlx::query_as(FLAG_STUCK_SQL)
        .bind(STUCK_AFTER_SECS)
        .bind(STUCK_BATCH)
        .bind(only)
        .fetch_all(pool)
        .await?;
    let minutes = STUCK_AFTER_SECS / 60;
    Ok(stuck
        .iter()
        .map(|lane| {
            lane_notice(
                lane,
                "inbox_stuck",
                format!("Inbox events waiting: {}", lane.key),
                format!(
                    "Events have been pending on this lane for over {minutes} minutes: its \
                     session may be blocked on a prompt, or delivery keeps failing."
                ),
            )
        })
        .collect())
}

pub fn alert<S: SessionsContext>(state: &S, notice: NewNotification) {
    state.alert(notice);
}

pub async fn sweep<S: SessionsContext>(state: &S) {
    match flag_stuck(state.pool(), None).await {
        Ok(notices) => {
            for notice in notices {
                alert(state, notice);
            }
        }
        Err(e) => tracing::warn!("inbox stuck-lane check failed: {e}"),
    }
}

/// `owner` is [`crate::auth::AuthContext::owner_filter`].
pub async fn set_paused(
    pool: &PgPool,
    owner: Option<Uuid>,
    lane_id: Uuid,
    paused: bool,
) -> sqlx::Result<Option<Lane>> {
    sqlx::query_as(
        "UPDATE inbox_lanes SET paused = $3, \
             paused_reason = CASE WHEN $3 THEN COALESCE(paused_reason, $4) END, \
             ceiling_from = CASE WHEN $3 THEN ceiling_from ELSE now() END, \
             stuck_alerted_at = CASE WHEN $3 THEN stuck_alerted_at END \
         WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2) \
         RETURNING id, key, preset_id, session_id, paused, paused_reason",
    )
    .bind(lane_id)
    .bind(owner)
    .bind(paused)
    .bind(MANUAL)
    .fetch_optional(pool)
    .await
}
