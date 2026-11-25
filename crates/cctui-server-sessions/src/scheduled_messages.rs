//! Messages scheduled into a session, stored as inbox events on its
//! `session:<id>` lane. Each one is its own batch under its turn id.

use cctui_proto::api::auto_followup::SendFollowup;
use chrono::{DateTime, Duration, Utc};

use crate::auth::AuthContext;
use crate::context::SessionsContext;
use crate::error::AppError;
use crate::inbox::claim;
use crate::inbox::deliver::Outcome;
use crate::outbox::{CLAIM_LEASE_SECS, Retry};
#[cfg(test)]
use crate::outbox::{MAX_ATTEMPTS, retry_after_failure};
use crate::store::sessions::SessionRowStatus;

pub const MAX_HORIZON_DAYS: i64 = 30;

#[derive(Debug, PartialEq, Eq)]
pub enum DeliverAtError {
    Malformed,
    Past,
    TooFar,
}

impl std::fmt::Display for DeliverAtError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Malformed => "deliver_at must be an RFC3339 timestamp",
            Self::Past => "deliver_at must be in the future",
            Self::TooFar => "deliver_at must be at most 30 days ahead",
        })
    }
}

pub fn parse_deliver_at(raw: &str, now: DateTime<Utc>) -> Result<DateTime<Utc>, DeliverAtError> {
    let at = DateTime::parse_from_rfc3339(raw.trim())
        .map_err(|_| DeliverAtError::Malformed)?
        .with_timezone(&Utc);
    if at <= now {
        return Err(DeliverAtError::Past);
    }
    if at > now + Duration::days(MAX_HORIZON_DAYS) {
        return Err(DeliverAtError::TooFar);
    }
    Ok(at)
}

/// Reason a message can never be delivered to a session in this status.
#[must_use]
pub fn undeliverable_reason(session_status: Option<&str>) -> Option<&'static str> {
    let Some(status) = session_status else {
        return Some("session no longer exists");
    };
    match SessionRowStatus::parse(status)? {
        SessionRowStatus::Ended => Some("session ended before delivery"),
        SessionRowStatus::Archived => Some("session archived before delivery"),
        SessionRowStatus::Failed => Some("session failed to launch"),
        _ => None,
    }
}

pub const SEAL_NEEDS_KEY: &str = "a message with <redacted> spans can only be scheduled when \
     the server has a vault key (CCTUI_VAULT_KEY)";
const UNSEALABLE: &str = "the sealed message could not be decrypted with the vault key";

/// The stored form of `content`: `<redacted>` spans masked in the shown body,
/// the real text kept only encrypted. `key` is read only when there is a span.
pub fn seal(
    content: &str,
    key: impl FnOnce() -> Vec<u8>,
) -> Result<(String, Option<String>), &'static str> {
    crate::routes::spawn::seal_redacted(content, key).map_err(|_| SEAL_NEEDS_KEY)
}

/// The text to deliver: the sealed original when there is one.
pub fn delivery_text(
    row: &ClaimedRow,
    key: impl FnOnce() -> Vec<u8>,
) -> Result<String, &'static str> {
    let Some(sealed) = &row.sealed_body else { return Ok(row.body.clone()) };
    let key = key();
    if key.is_empty() {
        return Err(UNSEALABLE);
    }
    crate::crypto::decrypt(sealed, &key).ok_or(UNSEALABLE)
}

pub const KIND: &str = "scheduled_message";

#[derive(sqlx::FromRow)]
pub struct ClaimedRow {
    pub id: uuid::Uuid,
    pub session_id: String,
    pub body: String,
    pub sealed_body: Option<String>,
    pub ask_picks: Option<serde_json::Value>,
    pub attempts: i32,
    pub turn_id: uuid::Uuid,
    pub user_id: uuid::Uuid,
    pub deliver_at: DateTime<Utc>,
    pub session_status: Option<String>,
    pub session_tempo: Option<String>,
}

const CLAIMED_SQL: &str = "\
    SELECT e.id, substr(l.key, 9) AS session_id, e.body, e.sealed_body, \
           e.payload->'ask_picks' AS ask_picks, e.attempts, e.batch_id AS turn_id, e.user_id, \
           e.not_before AS deliver_at, s.status AS session_status, s.tempo AS session_tempo \
    FROM inbox_events e \
    JOIN inbox_lanes l ON l.id = e.lane_id \
    LEFT JOIN sessions s ON s.id = l.session_id \
    WHERE e.batch_id = $1 AND e.status = 'claimed' AND e.payload->>'kind' = 'scheduled_message'";

const CLAIM_ONE_SQL: &str = "\
    WITH claimed AS ( \
        UPDATE inbox_events e \
        SET status = 'claimed', batch_id = COALESCE(e.batch_id, gen_random_uuid()), \
            next_attempt_at = now() + $3::bigint * interval '1 second' \
        FROM inbox_lanes l \
        WHERE l.id = e.lane_id AND e.id = $1 AND l.key = 'session:' || $2::text \
          AND e.status = 'pending' AND e.payload->>'kind' = 'scheduled_message' \
        RETURNING e.id, l.session_id AS lane_session, e.body, e.sealed_body, e.payload, \
                  e.attempts, e.batch_id, e.user_id, e.not_before) \
    SELECT c.id, $2::text AS session_id, c.body, c.sealed_body, \
           c.payload->'ask_picks' AS ask_picks, c.attempts, c.batch_id AS turn_id, c.user_id, \
           c.not_before AS deliver_at, s.status AS session_status, s.tempo AS session_tempo \
    FROM claimed c LEFT JOIN sessions s ON s.id = c.lane_session";

/// The scheduled message a lane worker just claimed, when it is the whole batch.
pub async fn claimed(
    pool: &sqlx::PgPool,
    events: &[crate::inbox::claim::BatchEvent],
) -> sqlx::Result<Option<ClaimedRow>> {
    let [event] = events else { return Ok(None) };
    sqlx::query_as(CLAIMED_SQL).bind(event.batch_id).fetch_optional(pool).await
}

pub async fn claim_one(
    pool: &sqlx::PgPool,
    session_id: &str,
    id: uuid::Uuid,
) -> sqlx::Result<Option<ClaimedRow>> {
    sqlx::query_as(CLAIM_ONE_SQL)
        .bind(id)
        .bind(session_id)
        .bind(CLAIM_LEASE_SECS)
        .fetch_optional(pool)
        .await
}

pub async fn scheduled_turns(
    pool: &sqlx::PgPool,
    session_id: &str,
) -> sqlx::Result<std::collections::HashMap<uuid::Uuid, DateTime<Utc>>> {
    let rows: Vec<(uuid::Uuid, DateTime<Utc>)> =
        sqlx::query_as("SELECT turn_id, scheduled_at FROM scheduled_turns WHERE session_id = $1")
            .bind(session_id)
            .fetch_all(pool)
            .await?;
    Ok(rows.into_iter().collect())
}

/// The lane worker's delivery: the wake is checked against the scheduler's
/// scopes at delivery time.
pub async fn deliver_due<S: SessionsContext>(state: &S, row: ClaimedRow) -> Outcome {
    match authorize_wake(state, &row).await {
        Ok(()) => deliver(state, row).await,
        Err(e) => {
            let (code, axum::Json(e)) = e.into_parts();
            if code.is_server_error() {
                Outcome::RetryKept(e.error)
            } else {
                tracing::warn!(event_id = %row.id, "scheduled message refused: {}", e.error);
                Outcome::Dead(e.error)
            }
        }
    }
}

async fn authorize_wake<S: SessionsContext>(state: &S, row: &ClaimedRow) -> Result<(), AppError> {
    let Some(status) = row.session_status.as_deref() else { return Ok(()) };
    if undeliverable_reason(Some(status)).is_some()
        || !crate::routes::sessions::reply_wakes_worker(status, row.session_tempo.as_deref())
    {
        return Ok(());
    }
    let ctx = AuthContext {
        user_id: row.user_id,
        key_id: uuid::Uuid::nil(),
        machine_id: None,
        scopes: crate::store::acls::user_ceiling(state.pool(), row.user_id).await?,
    };
    crate::routes::spawn::authorize_session_launch(state, &ctx, &row.session_id).await
}

/// Failures keep the batch, so a retry still carries the scheduled turn id.
pub async fn deliver<S: SessionsContext>(state: &S, mut row: ClaimedRow) -> Outcome {
    if let Some(reason) = undeliverable_reason(row.session_status.as_deref()) {
        return Outcome::Dead(reason.to_owned());
    }
    match crate::followup_op::in_progress(state.pool(), &row.session_id).await {
        Ok(false) => {}
        Ok(true) => return Outcome::RetryKept(crate::followup_op::BUSY.to_owned()),
        Err(e) => return Outcome::RetryKept(e.to_string()),
    }
    let text = match delivery_text(&row, crate::crypto::vault_key) {
        Ok(text) => text,
        Err(reason) => return Outcome::Dead(reason.to_owned()),
    };
    let followup = if row.ask_picks.is_some() { SendFollowup::Never } else { SendFollowup::Auto };
    let key = row.turn_id.to_string();
    match crate::auto_followup::route(state, None, &row.session_id, followup, &text, Some(&key))
        .await
    {
        Ok(None) => {}
        Ok(Some(followed)) => {
            return Outcome::Delivered {
                session_id: followed.to.unwrap_or(row.session_id),
                delivered_as: "followup",
            };
        }
        Err(e) => return Outcome::RetryKept(e.into_parts().1.0.error),
    }
    let env = state.resume_env_for_session(&row.session_id).await;
    let command_id = uuid::Uuid::new_v4();
    crate::commands::track_command(
        state.pending_commands(),
        command_id,
        Some(row.session_id.clone()),
        None,
    );
    let ask_picks = row.ask_picks.take().and_then(|v| serde_json::from_value(v).ok());
    let dispatch = crate::bus::dispatch(
        state,
        &row.session_id,
        cctui_proto::adapter::AdapterCommand::Reply {
            local_id: row.session_id.clone(),
            recover_auth: false,
            text,
            ask_picks,
            env,
            command_id: Some(command_id),
            turn_id: Some(row.turn_id),
        },
    )
    .await;
    match dispatch {
        Ok(()) => {
            if let Err(e) = record_turn(state.pool(), &row).await {
                tracing::warn!(event_id = %row.id, "scheduled turn not recorded: {e}");
            }
            Outcome::Delivered { session_id: row.session_id, delivered_as: "reply" }
        }
        Err(err) => Outcome::RetryKept(err.to_string()),
    }
}

async fn record_turn(pool: &sqlx::PgPool, row: &ClaimedRow) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO scheduled_turns (turn_id, session_id, event_id, scheduled_at) \
         VALUES ($1, $2, $3, $4) ON CONFLICT (turn_id) DO NOTHING",
    )
    .bind(row.turn_id)
    .bind(&row.session_id)
    .bind(row.id)
    .bind(row.deliver_at)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Settle a send-now claim, which no lane worker holds.
pub async fn settle(
    pool: &sqlx::PgPool,
    turn_id: uuid::Uuid,
    attempts: i32,
    outcome: Outcome,
) -> sqlx::Result<Result<(), String>> {
    Ok(match outcome {
        Outcome::Delivered { session_id, delivered_as } => {
            claim::mark_delivered(pool, turn_id, &session_id, delivered_as).await?;
            Ok(())
        }
        Outcome::Skip => {
            claim::defer(pool, turn_id).await?;
            Ok(())
        }
        Outcome::Dead(reason) => {
            claim::mark_dead(pool, turn_id, attempts + 1, &reason).await?;
            Err(reason)
        }
        Outcome::Retry(err) | Outcome::RetryKept(err) => {
            if let Retry::Dead { attempts } =
                claim::record_ambiguous_failure(pool, turn_id, attempts, &err).await?
            {
                tracing::error!(%turn_id, attempts, "scheduled message dead-lettered: {err}");
            }
            Err(err)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-09-24T10:00:00Z").unwrap().with_timezone(&Utc)
    }

    #[test]
    fn deliver_at_must_be_future_and_within_horizon() {
        assert_eq!(parse_deliver_at("nope", now()), Err(DeliverAtError::Malformed));
        assert_eq!(parse_deliver_at("2026-09-24T10:00:00Z", now()), Err(DeliverAtError::Past));
        assert_eq!(parse_deliver_at("2026-09-24T09:00:00Z", now()), Err(DeliverAtError::Past));
        assert_eq!(parse_deliver_at("2026-10-24T10:00:01Z", now()), Err(DeliverAtError::TooFar));
        assert_eq!(parse_deliver_at("2026-10-24T10:00:00Z", now()), Ok(now() + Duration::days(30)));
        assert_eq!(
            parse_deliver_at("2026-09-25T09:00:00+02:00", now()),
            Ok(now() + Duration::hours(21))
        );
    }

    /// Doubling 10 s → 1 h within the shared ±20% jitter, then dead-lettered once
    /// the attempt budget is spent.
    #[test]
    fn failures_back_off_then_dead_letter() {
        for (attempts, base) in [(0, 10), (1, 20), (2, 40), (6, 640)] {
            let (lo, hi) = (base * 8 / 10, base * 12 / 10);
            match retry_after_failure(attempts) {
                Retry::After { attempts: next, secs } => {
                    assert_eq!(next, attempts + 1);
                    assert!(
                        (lo..=hi).contains(&secs),
                        "attempts {attempts}: {secs}s outside {lo}..={hi}"
                    );
                }
                other @ Retry::Dead { .. } => {
                    panic!("attempts {attempts} should retry, got {other:?}")
                }
            }
        }
        assert_eq!(retry_after_failure(MAX_ATTEMPTS - 1), Retry::Dead { attempts: MAX_ATTEMPTS });
        assert_eq!(retry_after_failure(20), Retry::Dead { attempts: 21 });
    }

    /// Sum of the superseded fixed table (10/30/120/300/900/1800/3600). The
    /// doubling schedule must reach at least this far, even at the bottom of the
    /// jitter range, or an hour-long outage dead-letters where it used to recover.
    const PREVIOUS_HORIZON_SECS: i64 = 6760;

    #[test]
    fn retry_horizon_is_no_shorter_than_the_table_it_replaced() {
        let horizon: i64 = (0..MAX_ATTEMPTS - 1)
            .map(|attempts| match retry_after_failure(attempts) {
                Retry::After { secs, .. } => secs,
                Retry::Dead { .. } => panic!("attempts {attempts} is inside the budget"),
            })
            .sum();
        assert!(
            horizon >= PREVIOUS_HORIZON_SECS,
            "horizon {horizon}s is shorter than the {PREVIOUS_HORIZON_SECS}s it replaced"
        );
        assert!(horizon >= 3600, "horizon {horizon}s cannot outlast a one-hour outage");
        assert_eq!(retry_after_failure(MAX_ATTEMPTS - 1), Retry::Dead { attempts: MAX_ATTEMPTS });
    }

    #[test]
    fn ended_and_archived_sessions_are_undeliverable() {
        assert!(undeliverable_reason(Some("ended")).is_some());
        assert!(undeliverable_reason(Some("archived")).is_some());
        assert!(undeliverable_reason(Some("failed")).is_some());
        assert!(undeliverable_reason(None).is_some());
        assert_eq!(undeliverable_reason(Some("active")), None);
        assert_eq!(undeliverable_reason(Some("inactive")), None);
    }
}

#[cfg(test)]
mod reply_gate_tests {

    use crate::routes::sessions::reply_wakes_worker;

    #[test]
    fn only_a_gone_worker_or_a_stopped_session_is_woken() {
        assert!(reply_wakes_worker("inactive", Some("hibernated")));
        assert!(reply_wakes_worker("active", Some("dead")));
        assert!(reply_wakes_worker("ended", None));
        assert!(reply_wakes_worker("failed", None));
        assert!(reply_wakes_worker("archived", None));
        assert!(!reply_wakes_worker("active", Some("idle")));
        assert!(!reply_wakes_worker("inactive", Some("blocked")));
        assert!(!reply_wakes_worker("new", None));
        assert!(!reply_wakes_worker("draft", None));
    }
}
