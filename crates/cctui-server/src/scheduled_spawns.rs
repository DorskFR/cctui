//! Drafts queued to launch later. A claim is a lease, exactly as in
//! [`crate::scheduled_messages`]: a row left in `launching` by a crashed
//! replica becomes claimable again once its `next_attempt_at` passes.
//!
//! The horizon, the RFC3339 validation and the retry schedule are the
//! scheduled-message ones — the two outboxes must not drift apart.

use chrono::{DateTime, Utc};
use cctui_proto::ws::{ScheduledLaunchState, ServerEvent};

use crate::auth::{AuthContext, Scope};
use crate::bus::Bus;
use crate::scheduled_messages::{Retry, retry_after_failure};
use crate::state::AppState;
use crate::store::sessions::SessionRowStatus;

const CLAIM_LEASE_SECS: i64 = 300;
const CLAIM_BATCH: i64 = 50;

/// Reason a draft can never be launched.
pub fn unlaunchable_reason(draft_status: Option<&str>) -> Option<&'static str> {
    match draft_status {
        None => Some("draft no longer exists"),
        Some(status) if SessionRowStatus::parse(status) == Some(SessionRowStatus::Draft) => None,
        Some(_) => Some("session is no longer a draft"),
    }
}

/// Push the drafts list's view of one transition. `user_id` scopes delivery;
/// without it the event only reaches admins.
fn announce(
    bus: &Bus,
    draft_id: &str,
    user_id: Option<uuid::Uuid>,
    state: ScheduledLaunchState,
    launch_at: Option<DateTime<Utc>>,
    last_error: Option<&str>,
) {
    bus.publish_server(ServerEvent::ScheduledLaunch {
        draft_id: draft_id.to_owned(),
        user_id,
        state,
        launch_at,
        last_error: last_error.map(ToOwned::to_owned),
    });
}

#[derive(sqlx::FromRow)]
pub struct ClaimedDraft {
    pub draft_id: String,
    pub attempts: i32,
    pub draft_status: Option<String>,
    /// The machine's owner, whose accounts the spawn resolves against.
    pub owner: Option<uuid::Uuid>,
}

const CLAIM_SQL: &str = "\
    WITH claimed AS ( \
        UPDATE draft_launch_queue q \
        SET state = 'launching', next_attempt_at = now() + ($1 || ' seconds')::interval \
        WHERE q.draft_id IN ( \
            SELECT draft_id FROM draft_launch_queue \
            WHERE (state = 'scheduled' AND launch_at <= now() AND next_attempt_at <= now()) \
               OR (state = 'launching' AND next_attempt_at <= now()) \
            ORDER BY launch_at \
            LIMIT $2 \
            FOR UPDATE SKIP LOCKED) \
        RETURNING q.draft_id, q.attempts) \
    SELECT c.draft_id, c.attempts, s.status AS draft_status, mm.user_id AS owner \
    FROM claimed c \
    LEFT JOIN sessions s ON s.id = c.draft_id \
    LEFT JOIN machines mm ON mm.id = s.machine_uuid";

pub async fn claim_due(pool: &sqlx::PgPool) -> sqlx::Result<Vec<ClaimedDraft>> {
    sqlx::query_as(CLAIM_SQL)
        .bind(CLAIM_LEASE_SECS.to_string())
        .bind(CLAIM_BATCH)
        .fetch_all(pool)
        .await
}

/// Queue a draft to launch at `launch_at`, replacing any existing schedule.
pub async fn schedule(
    pool: &sqlx::PgPool,
    bus: &Bus,
    draft_id: &str,
    user_id: Option<uuid::Uuid>,
    launch_at: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO draft_launch_queue (draft_id, user_id, launch_at, next_attempt_at) \
         VALUES ($1, $2, $3, $3) \
         ON CONFLICT (draft_id) DO UPDATE \
         SET launch_at = $3, next_attempt_at = $3, state = 'scheduled', attempts = 0, \
             last_error = NULL, launched_at = NULL",
    )
    .bind(draft_id)
    .bind(user_id)
    .bind(launch_at)
    .execute(pool)
    .await?;
    announce(bus, draft_id, user_id, ScheduledLaunchState::Scheduled, Some(launch_at), None);
    Ok(())
}

/// Drop a draft's schedule, leaving the draft itself in place.
pub async fn cancel(pool: &sqlx::PgPool, bus: &Bus, draft_id: &str) -> sqlx::Result<bool> {
    let deleted: Option<(Option<uuid::Uuid>,)> =
        sqlx::query_as("DELETE FROM draft_launch_queue WHERE draft_id = $1 RETURNING user_id")
            .bind(draft_id)
            .fetch_optional(pool)
            .await?;
    let Some((user_id,)) = deleted else { return Ok(false) };
    announce(bus, draft_id, user_id, ScheduledLaunchState::Cancelled, None, None);
    Ok(true)
}

/// `(launch_at, last_error)` per scheduled draft, for the drafts list.
pub async fn pending_for(
    pool: &sqlx::PgPool,
    draft_ids: &[String],
) -> sqlx::Result<std::collections::HashMap<String, (DateTime<Utc>, Option<String>)>> {
    let rows: Vec<(String, DateTime<Utc>, Option<String>)> = sqlx::query_as(
        "SELECT draft_id, launch_at, last_error FROM draft_launch_queue \
         WHERE draft_id = ANY($1) AND state <> 'launched'",
    )
    .bind(draft_ids)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(id, at, err)| (id, (at, err))).collect())
}

pub async fn sweep(state: &AppState) {
    let rows = match claim_due(&state.pool).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!("scheduled spawn claim failed: {e}");
            return;
        }
    };
    for row in rows {
        let _ = launch(state, row).await;
    }
}

pub async fn launch(state: &AppState, row: ClaimedDraft) -> Result<(), String> {
    if let Some(reason) = unlaunchable_reason(row.draft_status.as_deref()) {
        mark_dead(&state.pool, &state.bus, &row.draft_id, row.owner, row.attempts + 1, reason).await;
        return Err(reason.to_owned());
    }
    let Some(owner) = row.owner else {
        let reason = "draft's machine no longer exists";
        mark_dead(&state.pool, &state.bus, &row.draft_id, None, row.attempts + 1, reason).await;
        return Err(reason.to_owned());
    };
    // No env: a draft never stores secrets, and nobody is at the keyboard to
    // re-enter them. Account gateway env is still minted inside the dispatch.
    let ctx = AuthContext {
        user_id: owner,
        key_id: uuid::Uuid::nil(),
        machine_id: None,
        scopes: [Scope::Read, Scope::Dispatch].into_iter().collect(),
    };
    match crate::routes::spawn::launch_stored_draft(
        state,
        &ctx,
        &row.draft_id,
        std::collections::BTreeMap::new(),
    )
    .await
    {
        Ok(_) => {
            mark_launched(&state.pool, &state.bus, &row.draft_id, Some(owner)).await;
            tracing::info!(draft = %row.draft_id, "scheduled draft launched");
            Ok(())
        }
        Err(e) => {
            let err = e.to_string();
            record_failure(&state.pool, &state.bus, &row.draft_id, Some(owner), row.attempts, &err)
                .await;
            Err(err)
        }
    }
}

async fn mark_launched(
    pool: &sqlx::PgPool,
    bus: &Bus,
    draft_id: &str,
    user_id: Option<uuid::Uuid>,
) {
    let _ = sqlx::query(
        "UPDATE draft_launch_queue \
         SET state = 'launched', launched_at = now(), last_error = NULL \
         WHERE draft_id = $1",
    )
    .bind(draft_id)
    .execute(pool)
    .await;
    announce(bus, draft_id, user_id, ScheduledLaunchState::Launched, None, None);
}

pub async fn record_failure(
    pool: &sqlx::PgPool,
    bus: &Bus,
    draft_id: &str,
    user_id: Option<uuid::Uuid>,
    attempts: i32,
    err: &str,
) {
    match retry_after_failure(attempts) {
        Retry::Dead { attempts } => {
            mark_dead(pool, bus, draft_id, user_id, attempts, err).await;
            tracing::error!(draft = %draft_id, attempts, "scheduled spawn dead-lettered: {err}");
        }
        Retry::After { attempts, secs } => {
            let _ = sqlx::query(
                "UPDATE draft_launch_queue \
                 SET state = 'scheduled', attempts = $2, last_error = $3, \
                     next_attempt_at = now() + ($4 || ' seconds')::interval \
                 WHERE draft_id = $1",
            )
            .bind(draft_id)
            .bind(attempts)
            .bind(err)
            .bind(secs.to_string())
            .execute(pool)
            .await;
            announce(bus, draft_id, user_id, ScheduledLaunchState::Failed, None, Some(err));
            tracing::warn!(draft = %draft_id, attempt = attempts, retry_in_secs = secs, "scheduled spawn launch failed: {err}");
        }
    }
}

async fn mark_dead(
    pool: &sqlx::PgPool,
    bus: &Bus,
    draft_id: &str,
    user_id: Option<uuid::Uuid>,
    attempts: i32,
    reason: &str,
) {
    let _ = sqlx::query(
        "UPDATE draft_launch_queue SET state = 'dead', attempts = $2, last_error = $3 \
         WHERE draft_id = $1",
    )
    .bind(draft_id)
    .bind(attempts)
    .bind(reason)
    .execute(pool)
    .await;
    announce(bus, draft_id, user_id, ScheduledLaunchState::Dead, None, Some(reason));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::NoopTransport;
    use crate::scheduled_messages::{DeliverAtError, MAX_HORIZON_DAYS, parse_deliver_at};

    fn test_bus() -> Bus {
        Bus::new(Box::new(NoopTransport))
    }

    /// The `(state, user_id, launch_at, last_error)` of the next scheduled-launch
    /// event for `draft_id`, or `None` if the stream holds none.
    #[allow(clippy::type_complexity)]
    fn next_launch_event(
        rx: &mut tokio::sync::broadcast::Receiver<crate::bus::ServerFrame>,
        draft_id: &str,
    ) -> Option<(ScheduledLaunchState, Option<uuid::Uuid>, Option<DateTime<Utc>>, Option<String>)>
    {
        while let Ok(frame) = rx.try_recv() {
            if let ServerEvent::ScheduledLaunch { draft_id: id, user_id, state, launch_at, last_error } =
                &*frame.event
                && id == draft_id
            {
                return Some((*state, *user_id, *launch_at, last_error.clone()));
            }
        }
        None
    }

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-09-24T10:00:00Z").unwrap().with_timezone(&Utc)
    }

    #[test]
    fn launch_at_shares_the_scheduled_message_window() {
        assert_eq!(parse_deliver_at("soon", now()), Err(DeliverAtError::Malformed));
        assert_eq!(parse_deliver_at("2026-09-24T09:59:59Z", now()), Err(DeliverAtError::Past));
        assert_eq!(parse_deliver_at("2026-10-24T10:00:01Z", now()), Err(DeliverAtError::TooFar));
        assert_eq!(
            parse_deliver_at("2026-10-24T10:00:00Z", now()),
            Ok(now() + chrono::Duration::days(MAX_HORIZON_DAYS))
        );
    }

    #[test]
    fn a_discarded_or_promoted_draft_is_unlaunchable() {
        assert_eq!(unlaunchable_reason(Some("draft")), None);
        assert!(unlaunchable_reason(None).is_some());
        assert!(unlaunchable_reason(Some("active")).is_some());
        assert!(unlaunchable_reason(Some("ended")).is_some());
    }

    async fn seed_draft(pool: &sqlx::PgPool, name: &str) -> String {
        seed_owned_draft(pool, name).await.0
    }

    async fn seed_owned_draft(pool: &sqlx::PgPool, name: &str) -> (String, uuid::Uuid) {
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
        let sid = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status, \
             adapter_id) VALUES ($1, $2, $2, $3, '/w', 'draft', 'claude-code')",
        )
        .bind(&sid)
        .bind(machine)
        .bind(uid)
        .execute(pool)
        .await
        .unwrap();
        (sid, uid)
    }

    #[tokio::test]
    async fn due_drafts_are_claimed_once_then_back_off_and_dead_letter() {
        let name = "scheduled_spawn_claim";
        let Some(url) = crate::routes::gateway::test_db_url(name) else { return };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .connect(&url)
            .await
            .expect("connect test db");
        let due = seed_draft(&pool, name).await;
        let later = seed_draft(&pool, name).await;
        let bus = test_bus();
        schedule(&pool, &bus, &due, None, now()).await.unwrap();
        schedule(&pool, &bus, &later, None, Utc::now() + chrono::Duration::hours(1)).await.unwrap();

        let (a, b) = tokio::join!(claim_due(&pool), claim_due(&pool));
        let mine: Vec<String> = a
            .unwrap()
            .into_iter()
            .chain(b.unwrap())
            .map(|r| r.draft_id)
            .filter(|id| id == &due || id == &later)
            .collect();
        assert_eq!(mine, vec![due.clone()], "the due draft is claimed once, the future one not");
        assert!(claim_due(&pool).await.unwrap().iter().all(|r| r.draft_id != due), "leased");

        record_failure(&pool, &bus, &due, None, 0, "machine offline").await;
        let (state, attempts, err): (String, i32, Option<String>) = sqlx::query_as(
            "SELECT state, attempts, last_error FROM draft_launch_queue WHERE draft_id = $1",
        )
        .bind(&due)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            (state.as_str(), attempts, err.as_deref()),
            ("scheduled", 1, Some("machine offline"))
        );
        assert!(claim_due(&pool).await.unwrap().iter().all(|r| r.draft_id != due), "backing off");

        record_failure(&pool, &bus, &due, None, 10, "still offline").await;
        let state: String =
            sqlx::query_scalar("SELECT state FROM draft_launch_queue WHERE draft_id = $1")
                .bind(&due)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(state, "dead", "a dead-lettered schedule keeps its draft");
        let still_there: i64 =
            sqlx::query_scalar("SELECT count(*) FROM sessions WHERE id = $1 AND status = 'draft'")
                .bind(&due)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(still_there, 1);

        let pending = pending_for(&pool, &[due.clone(), later.clone()]).await.unwrap();
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[&due].1.as_deref(), Some("still offline"));

        assert!(cancel(&pool, &bus, &later).await.unwrap());
        assert!(!cancel(&pool, &bus, &later).await.unwrap());
        assert!(pending_for(&pool, &[later]).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn discarding_a_draft_takes_its_schedule_with_it() {
        let name = "scheduled_spawn_cascade";
        let Some(url) = crate::routes::gateway::test_db_url(name) else { return };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let draft = seed_draft(&pool, name).await;
        schedule(&pool, &test_bus(), &draft, None, Utc::now() + chrono::Duration::hours(2))
            .await
            .unwrap();
        sqlx::query("DELETE FROM sessions WHERE id = $1")
            .bind(&draft)
            .execute(&pool)
            .await
            .unwrap();
        assert!(pending_for(&pool, &[draft]).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn every_transition_pushes_a_scheduled_launch_event() {
        let name = "scheduled_spawn_events";
        let Some(url) = crate::routes::gateway::test_db_url(name) else { return };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let bus = test_bus();
        let mut rx = bus.subscribe_server();
        let (draft, owner) = seed_owned_draft(&pool, name).await;
        let at = Utc::now() + chrono::Duration::hours(3);

        schedule(&pool, &bus, &draft, Some(owner), at).await.unwrap();
        let (state, user, launch_at, err) = next_launch_event(&mut rx, &draft).expect("scheduled");
        assert_eq!((state, user, err), (ScheduledLaunchState::Scheduled, Some(owner), None));
        assert_eq!(launch_at, Some(at), "the schedule rides along so no refetch is needed");

        assert!(cancel(&pool, &bus, &draft).await.unwrap());
        let (state, user, ..) = next_launch_event(&mut rx, &draft).expect("cancelled");
        assert_eq!((state, user), (ScheduledLaunchState::Cancelled, Some(owner)));
        assert!(
            next_launch_event(&mut rx, &draft).is_none(),
            "cancelling an unscheduled draft is silent"
        );
        assert!(!cancel(&pool, &bus, &draft).await.unwrap());
        assert!(next_launch_event(&mut rx, &draft).is_none());

        schedule(&pool, &bus, &draft, Some(owner), at).await.unwrap();
        next_launch_event(&mut rx, &draft).expect("rescheduled");

        record_failure(&pool, &bus, &draft, Some(owner), 0, "machine offline").await;
        let (state, user, _, err) = next_launch_event(&mut rx, &draft).expect("failed");
        assert_eq!(
            (state, user, err.as_deref()),
            (ScheduledLaunchState::Failed, Some(owner), Some("machine offline"))
        );

        record_failure(&pool, &bus, &draft, Some(owner), 10, "still offline").await;
        let (state, user, _, err) = next_launch_event(&mut rx, &draft).expect("dead");
        assert_eq!(
            (state, user, err.as_deref()),
            (ScheduledLaunchState::Dead, Some(owner), Some("still offline"))
        );

        mark_launched(&pool, &bus, &draft, Some(owner)).await;
        let (state, user, _, err) = next_launch_event(&mut rx, &draft).expect("launched");
        assert_eq!((state, user, err), (ScheduledLaunchState::Launched, Some(owner), None));
        let row: String =
            sqlx::query_scalar("SELECT state FROM draft_launch_queue WHERE draft_id = $1")
                .bind(&draft)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row, "launched");
    }
}
