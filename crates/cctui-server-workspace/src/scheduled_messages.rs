//! A claim is a lease: a row left in `sending` by a crashed replica becomes
//! claimable again once its `next_attempt_at` passes.

use std::time::Duration as StdDuration;

use axum::http::StatusCode;
use cctui_proto::backoff::Backoff;
use chrono::{DateTime, Duration, Utc};

use crate::auth::AuthContext;
use crate::error::AppError;
use crate::state::AppState;
use crate::store::sessions::SessionRowStatus;

pub const MAX_HORIZON_DAYS: i64 = 30;
/// Sized so the doubling schedule below still spans over two hours: a session
/// unreachable for an hour must recover, not dead-letter.
const MAX_ATTEMPTS: i32 = 11;

/// Redelivery schedule: 10 s doubling to a 1 h cap, jittered so a batch that
/// failed together does not retry in lockstep. The first retry stays seconds
/// away for a transient failure; [`MAX_ATTEMPTS`] carries the total reach.
const fn retry_schedule() -> Backoff {
    Backoff::new(StdDuration::from_secs(10), StdDuration::from_hours(1))
}
const CLAIM_LEASE_SECS: i64 = 300;
const CLAIM_BATCH: i64 = 50;

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

#[derive(Debug, PartialEq, Eq)]
pub enum Retry {
    After { attempts: i32, secs: i64 },
    Dead { attempts: i32 },
}

pub fn retry_after_failure(attempts: i32) -> Retry {
    let next = attempts + 1;
    if next >= MAX_ATTEMPTS {
        return Retry::Dead { attempts: next };
    }
    let secs = retry_schedule().delay_secs_for(u32::try_from(attempts).unwrap_or(u32::MAX));
    Retry::After { attempts: next, secs }
}

/// Reason a message can never be delivered to a session in this status.
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

#[derive(sqlx::FromRow)]
pub struct ClaimedRow {
    pub id: uuid::Uuid,
    pub session_id: String,
    pub body: String,
    pub ask_picks: Option<serde_json::Value>,
    pub attempts: i32,
    pub turn_id: uuid::Uuid,
    pub user_id: Option<uuid::Uuid>,
    pub session_status: Option<String>,
    pub session_tempo: Option<String>,
}

const CLAIM_SQL: &str = "\
    WITH claimed AS ( \
        UPDATE session_message_queue q \
        SET state = 'sending', next_attempt_at = now() + ($1 || ' seconds')::interval \
        WHERE q.id IN ( \
            SELECT id FROM session_message_queue \
            WHERE (state = 'scheduled' AND deliver_at <= now() AND next_attempt_at <= now()) \
               OR (state = 'sending' AND next_attempt_at <= now()) \
            ORDER BY deliver_at \
            LIMIT $2 \
            FOR UPDATE SKIP LOCKED) \
        RETURNING q.id, q.session_id, q.body, q.ask_picks, q.attempts, q.turn_id, q.user_id) \
    SELECT c.*, s.status AS session_status, s.tempo AS session_tempo \
    FROM claimed c LEFT JOIN sessions s ON s.id = c.session_id";

const CLAIM_ONE_SQL: &str = "\
    WITH claimed AS ( \
        UPDATE session_message_queue \
        SET state = 'sending', next_attempt_at = now() + ($3 || ' seconds')::interval \
        WHERE id = $1 AND session_id = $2 AND state = 'scheduled' \
        RETURNING id, session_id, body, ask_picks, attempts, turn_id, user_id) \
    SELECT c.*, s.status AS session_status, s.tempo AS session_tempo \
    FROM claimed c LEFT JOIN sessions s ON s.id = c.session_id";

pub async fn claim_due(pool: &sqlx::PgPool) -> sqlx::Result<Vec<ClaimedRow>> {
    sqlx::query_as(CLAIM_SQL)
        .bind(CLAIM_LEASE_SECS.to_string())
        .bind(CLAIM_BATCH)
        .fetch_all(pool)
        .await
}

pub async fn claim_one(
    pool: &sqlx::PgPool,
    session_id: &str,
    id: uuid::Uuid,
) -> sqlx::Result<Option<ClaimedRow>> {
    sqlx::query_as(CLAIM_ONE_SQL)
        .bind(id)
        .bind(session_id)
        .bind(CLAIM_LEASE_SECS.to_string())
        .fetch_optional(pool)
        .await
}

pub async fn scheduled_turns(
    pool: &sqlx::PgPool,
    session_id: &str,
) -> sqlx::Result<std::collections::HashMap<uuid::Uuid, DateTime<Utc>>> {
    let rows: Vec<(uuid::Uuid, DateTime<Utc>)> = sqlx::query_as(
        "SELECT turn_id, deliver_at FROM session_message_queue \
         WHERE session_id = $1 AND state = 'sent'",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().collect())
}

pub async fn sweep(state: &AppState) {
    let rows = match claim_due(&state.pool).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!("scheduled message claim failed: {e}");
            return;
        }
    };
    for row in rows {
        deliver_due(state, row).await;
    }
}

async fn deliver_due(state: &AppState, row: ClaimedRow) {
    match authorize_wake(state, &row).await {
        Ok(()) => {
            let _ = deliver(state, row).await;
        }
        Err(e) => {
            let (code, axum::Json(e)) = e.into_parts();
            if code.is_server_error() {
                record_failure(&state.pool, row.id, row.attempts, &e.error).await;
            } else {
                tracing::warn!(queue_id = %row.id, "scheduled message refused: {}", e.error);
                mark_dead(&state.pool, row.id, row.attempts + 1, &e.error).await;
            }
        }
    }
}

/// A due message that wakes its worker is sent for whoever scheduled it, with
/// the scopes they hold at delivery time.
async fn authorize_wake(state: &AppState, row: &ClaimedRow) -> Result<(), AppError> {
    let Some(status) = row.session_status.as_deref() else { return Ok(()) };
    if undeliverable_reason(Some(status)).is_some()
        || !crate::routes::sessions::reply_wakes_worker(status, row.session_tempo.as_deref())
    {
        return Ok(());
    }
    let Some(user_id) = row.user_id else {
        return Err(AppError::new(StatusCode::FORBIDDEN, "scheduled by an unknown user"));
    };
    let ctx = AuthContext {
        user_id,
        key_id: uuid::Uuid::nil(),
        machine_id: None,
        scopes: crate::store::acls::user_ceiling(&state.pool, user_id).await?,
    };
    crate::routes::spawn::authorize_session_launch(state, &ctx, &row.session_id).await
}

pub async fn deliver(state: &AppState, row: ClaimedRow) -> Result<(), String> {
    if let Some(reason) = undeliverable_reason(row.session_status.as_deref()) {
        mark_dead(&state.pool, row.id, row.attempts + 1, reason).await;
        return Err(reason.to_owned());
    }
    let env = crate::routes::gateway::resume_env_for_session(state, &row.session_id).await;
    let command_id = uuid::Uuid::new_v4();
    crate::state::track_command(
        &state.pending_commands,
        command_id,
        Some(row.session_id.clone()),
        None,
    );
    let ask_picks = row.ask_picks.and_then(|v| serde_json::from_value(v).ok());
    let dispatch = crate::bus::dispatch(
        state,
        &row.session_id,
        cctui_proto::adapter::AdapterCommand::Reply {
            local_id: row.session_id.clone(),
            recover_auth: false,
            text: row.body,
            ask_picks,
            env,
            command_id: Some(command_id),
            turn_id: Some(row.turn_id),
        },
    )
    .await;
    match dispatch {
        Ok(()) => {
            let _ = sqlx::query(
                "UPDATE session_message_queue \
                 SET state = 'sent', sent_at = now(), last_error = NULL WHERE id = $1",
            )
            .bind(row.id)
            .execute(&state.pool)
            .await;
            Ok(())
        }
        Err(err) => {
            let err = err.to_string();
            record_failure(&state.pool, row.id, row.attempts, &err).await;
            Err(err)
        }
    }
}

pub async fn record_failure(pool: &sqlx::PgPool, id: uuid::Uuid, attempts: i32, err: &str) {
    match retry_after_failure(attempts) {
        Retry::Dead { attempts } => {
            mark_dead(pool, id, attempts, err).await;
            tracing::error!(queue_id = %id, attempts, "scheduled message dead-lettered: {err}");
        }
        Retry::After { attempts, secs } => {
            let _ = sqlx::query(
                "UPDATE session_message_queue \
                 SET state = 'scheduled', attempts = $2, last_error = $3, \
                     next_attempt_at = now() + ($4 || ' seconds')::interval \
                 WHERE id = $1",
            )
            .bind(id)
            .bind(attempts)
            .bind(err)
            .bind(secs.to_string())
            .execute(pool)
            .await;
            tracing::warn!(queue_id = %id, attempt = attempts, retry_in_secs = secs, "scheduled message delivery failed: {err}");
        }
    }
}

async fn mark_dead(pool: &sqlx::PgPool, id: uuid::Uuid, attempts: i32, reason: &str) {
    let _ = sqlx::query(
        "UPDATE session_message_queue SET state = 'dead', attempts = $2, last_error = $3 \
         WHERE id = $1",
    )
    .bind(id)
    .bind(attempts)
    .bind(reason)
    .execute(pool)
    .await;
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

    async fn seed(pool: &sqlx::PgPool, name: &str) -> String {
        let uid = uuid::Uuid::new_v4();
        let machine = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("{name}-{uid}"))
            .bind(format!("h-{uid}"))
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, 'm', $3)")
            .bind(machine)
            .bind(uid)
            .bind(format!("mk-{machine}"))
            .execute(pool)
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
        .execute(pool)
        .await
        .unwrap();
        sid
    }

    async fn insert(pool: &sqlx::PgPool, sid: &str, due: bool) -> uuid::Uuid {
        let offset = if due { "-1 minute" } else { "1 hour" };
        sqlx::query_scalar(
            "INSERT INTO session_message_queue (session_id, body, deliver_at, next_attempt_at) \
             VALUES ($1, 'hi', now() + $2::interval, now() + $2::interval) RETURNING id",
        )
        .bind(sid)
        .bind(offset)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn due_rows_are_claimed_exactly_once_and_backoff_releases_them() {
        let name = "scheduled_claim_exactly_once";
        let Some(url) = crate::routes::gateway::test_db_url(name) else { return };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .connect(&url)
            .await
            .expect("connect test db");
        let sid = seed(&pool, name).await;
        let due = insert(&pool, &sid, true).await;
        let later = insert(&pool, &sid, false).await;

        let (a, b) = tokio::join!(claim_due(&pool), claim_due(&pool));
        let mine: Vec<uuid::Uuid> = a
            .unwrap()
            .into_iter()
            .chain(b.unwrap())
            .filter(|r| r.session_id == sid)
            .map(|r| r.id)
            .collect();
        assert_eq!(mine, vec![due], "the due row is claimed once, the future row not at all");
        assert!(claim_due(&pool).await.unwrap().iter().all(|r| r.id != due), "leased");
        assert!(
            claim_one(&pool, &sid, due).await.unwrap().is_none(),
            "send-now skips a claimed row"
        );

        record_failure(&pool, due, 0, "no daemon").await;
        let (st, attempts, err): (String, i32, Option<String>) = sqlx::query_as(
            "SELECT state, attempts, last_error FROM session_message_queue WHERE id = $1",
        )
        .bind(due)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!((st.as_str(), attempts, err.as_deref()), ("scheduled", 1, Some("no daemon")));
        assert!(claim_due(&pool).await.unwrap().iter().all(|r| r.id != due), "backing off");

        record_failure(&pool, due, MAX_ATTEMPTS - 1, "still offline").await;
        let st: String =
            sqlx::query_scalar("SELECT state FROM session_message_queue WHERE id = $1")
                .bind(due)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(st, "dead");

        let one = claim_one(&pool, &sid, later).await.unwrap().expect("send-now claims early");
        assert_eq!(one.session_status.as_deref(), Some("active"));
    }
}

#[cfg(test)]
mod reply_gate_tests {
    use axum::extract::{Path, State};
    use axum::http::StatusCode;
    use axum::{Extension, Json};
    use cctui_proto::adapter::AdapterCommand;
    use cctui_proto::ws::DaemonFrameDown;
    use tokio::sync::mpsc::Receiver;
    use uuid::Uuid;

    use super::{claim_one, deliver_due};
    use crate::auth::{AuthContext, Scope};
    use crate::routes::sessions::reply_wakes_worker;
    use crate::state::AppState;

    const READ_ONLY: &[Scope] = &[Scope::Read];
    const DISPATCH: &[Scope] = &[Scope::Read, Scope::Dispatch];

    fn caller(user_id: Uuid, scopes: &[Scope]) -> AuthContext {
        AuthContext {
            user_id,
            key_id: Uuid::new_v4(),
            machine_id: None,
            scopes: scopes.iter().copied().collect(),
        }
    }

    async fn state(name: &str) -> Option<AppState> {
        let url = crate::routes::gateway::test_db_url(name)?;
        let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
        Some(AppState::for_test(pool))
    }

    async fn seed_owner(pool: &sqlx::PgPool, ceiling: &[Scope]) -> (Uuid, Uuid) {
        let uid = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("reply-gate-{uid}"))
            .bind(format!("hrg-{uid}"))
            .execute(pool)
            .await
            .unwrap();
        set_ceiling(pool, uid, ceiling).await;
        let machine = Uuid::new_v4();
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, 'm', $3)")
            .bind(machine)
            .bind(uid)
            .bind(format!("mkrg-{machine}"))
            .execute(pool)
            .await
            .unwrap();
        (uid, machine)
    }

    async fn set_ceiling(pool: &sqlx::PgPool, uid: Uuid, scopes: &[Scope]) {
        let mut conn = pool.acquire().await.unwrap();
        crate::store::acls::set_user_ceiling(&mut conn, uid, scopes).await.unwrap();
    }

    async fn seed_session(pool: &sqlx::PgPool, owner: Uuid, machine: Uuid, tempo: &str) -> String {
        let sid = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status, \
             adapter_id, tempo) VALUES ($1, $2, $3, $4, '/w', 'inactive', 'claude-code', $5)",
        )
        .bind(&sid)
        .bind(machine.to_string())
        .bind(machine)
        .bind(owner)
        .bind(tempo)
        .execute(pool)
        .await
        .unwrap();
        sid
    }

    fn daemon(state: &AppState, machine: Uuid) -> Receiver<DaemonFrameDown> {
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        state.bus.register_daemon(machine, Uuid::new_v4(), tx);
        rx
    }

    fn replied(rx: &mut Receiver<DaemonFrameDown>) -> bool {
        matches!(
            rx.try_recv(),
            Ok(DaemonFrameDown::Command { command, .. })
                if matches!(*command, AdapterCommand::Reply { .. })
        )
    }

    async fn reply(state: &AppState, ctx: AuthContext, sid: &str) -> Result<(), StatusCode> {
        crate::routes::sessions::send_message(
            State(state.clone()),
            Extension(ctx),
            Path(sid.to_owned()),
            Json(serde_json::from_value(serde_json::json!({ "content": "hi" })).unwrap()),
        )
        .await
        .map(|_| ())
        .map_err(|e| e.into_parts().0)
    }

    async fn queue(pool: &sqlx::PgPool, sid: &str, user: Uuid) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO session_message_queue \
               (session_id, user_id, body, deliver_at, next_attempt_at) \
             VALUES ($1, $2, 'later', now() + interval '1 hour', now() + interval '1 hour') \
             RETURNING id",
        )
        .bind(sid)
        .bind(user)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn queue_state(pool: &sqlx::PgPool, id: Uuid) -> (String, Option<String>) {
        sqlx::query_as("SELECT state, last_error FROM session_message_queue WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

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

    #[tokio::test]
    async fn a_reply_that_wakes_the_worker_needs_dispatch_a_live_one_does_not() {
        let name = "a_reply_that_wakes_the_worker_needs_dispatch_a_live_one_does_not";
        let Some(state) = state(name).await else { return };
        let pool = state.pool.clone();
        let (owner, machine) = seed_owner(&pool, DISPATCH).await;
        let mut rx = daemon(&state, machine);
        let asleep = seed_session(&pool, owner, machine, "hibernated").await;
        let live = seed_session(&pool, owner, machine, "idle").await;

        let refused = reply(&state, caller(owner, READ_ONLY), &asleep).await;
        assert_eq!(refused, Err(StatusCode::FORBIDDEN));
        assert!(rx.try_recv().is_err(), "a refused reply never reaches the daemon");

        let scheduled = queue(&pool, &asleep, owner).await;
        let send_now = crate::routes::scheduled_messages::send_now(
            State(state.clone()),
            Extension(caller(owner, READ_ONLY)),
            Path((asleep.clone(), scheduled)),
        )
        .await
        .map_err(|e| e.into_parts().0);
        assert_eq!(send_now, Err(StatusCode::FORBIDDEN));
        assert!(rx.try_recv().is_err());
        assert_eq!(queue_state(&pool, scheduled).await.0, "scheduled", "left for its owner");

        assert_eq!(reply(&state, caller(owner, DISPATCH), &asleep).await, Ok(()));
        assert!(replied(&mut rx), "a dispatch key wakes the worker");

        assert_eq!(reply(&state, caller(owner, READ_ONLY), &live).await, Ok(()));
        assert!(replied(&mut rx), "typing into a live session is unchanged");
    }

    #[tokio::test]
    async fn a_due_wake_is_refused_once_its_scheduler_lost_dispatch() {
        let name = "a_due_wake_is_refused_once_its_scheduler_lost_dispatch";
        let Some(state) = state(name).await else { return };
        let pool = state.pool.clone();
        let (owner, machine) = seed_owner(&pool, DISPATCH).await;
        let mut rx = daemon(&state, machine);
        let asleep = seed_session(&pool, owner, machine, "hibernated").await;
        let live = seed_session(&pool, owner, machine, "idle").await;
        let to_asleep = queue(&pool, &asleep, owner).await;
        let to_live = queue(&pool, &live, owner).await;
        set_ceiling(&pool, owner, READ_ONLY).await;

        let row = claim_one(&pool, &asleep, to_asleep).await.unwrap().expect("claimed");
        deliver_due(&state, row).await;
        let (st, err) = queue_state(&pool, to_asleep).await;
        assert_eq!(st, "dead");
        assert!(err.is_some_and(|e| e.contains("dispatch")), "the refusal is recorded");
        assert!(rx.try_recv().is_err(), "the worker is not woken");

        let row = claim_one(&pool, &live, to_live).await.unwrap().expect("claimed");
        deliver_due(&state, row).await;
        assert_eq!(queue_state(&pool, to_live).await.0, "sent", "a live worker still gets it");
        assert!(replied(&mut rx));

        set_ceiling(&pool, owner, DISPATCH).await;
        let again = queue(&pool, &asleep, owner).await;
        let row = claim_one(&pool, &asleep, again).await.unwrap().expect("claimed");
        deliver_due(&state, row).await;
        assert_eq!(queue_state(&pool, again).await.0, "sent");
        assert!(replied(&mut rx), "with dispatch the due message wakes the worker");
    }
}
