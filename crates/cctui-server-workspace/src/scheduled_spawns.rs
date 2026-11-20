//! Drafts queued to launch later: one `timer` event per draft, on a
//! `draft:<id>` lane, keyed by the draft id so a reschedule replaces it.

use cctui_proto::ws::{ScheduledLaunchState, ServerEvent};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::auth::{AuthContext, Scope};
use crate::bus::Bus;
use crate::inbox::claim::BatchEvent;
use crate::inbox::deliver::Outcome;
use crate::state::AppState;
use crate::store::sessions::SessionRowStatus;

pub const LANE_PREFIX: &str = "draft:";
const EVENT_BODY: &str = "launch the scheduled draft";
const GONE: &str = "draft no longer exists";

#[must_use]
pub fn lane_key(draft_id: &str) -> String {
    format!("{LANE_PREFIX}{draft_id}")
}

/// Reason a draft can never be launched.
pub fn unlaunchable_reason(draft_status: Option<&str>) -> Option<&'static str> {
    match draft_status {
        None => Some(GONE),
        Some(status) if SessionRowStatus::parse(status) == Some(SessionRowStatus::Draft) => None,
        Some(_) => Some("session is no longer a draft"),
    }
}

/// Push the drafts list's view of one transition. `user_id` scopes delivery;
/// without it the event only reaches admins.
fn announce(
    bus: &Bus,
    draft_id: &str,
    user_id: Option<Uuid>,
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

/// `(status, machine owner)` of the draft row, `None` once it is gone.
async fn draft_facts(
    pool: &sqlx::PgPool,
    draft_id: &str,
) -> sqlx::Result<Option<(String, Option<Uuid>)>> {
    sqlx::query_as(
        "SELECT s.status, m.user_id FROM sessions s \
         LEFT JOIN machines m ON m.id = s.machine_uuid WHERE s.id = $1",
    )
    .bind(draft_id)
    .fetch_optional(pool)
    .await
}

async fn lane(pool: &sqlx::PgPool, owner: Uuid, draft_id: &str) -> sqlx::Result<Uuid> {
    sqlx::query_scalar(
        "INSERT INTO inbox_lanes (user_id, key) VALUES ($1, $2) \
         ON CONFLICT (user_id, key) DO UPDATE SET key = EXCLUDED.key RETURNING id",
    )
    .bind(owner)
    .bind(lane_key(draft_id))
    .fetch_one(pool)
    .await
}

/// Queue a draft to launch at `launch_at`, replacing any existing schedule.
pub async fn schedule(
    pool: &sqlx::PgPool,
    bus: &Bus,
    draft_id: &str,
    user_id: Option<Uuid>,
    launch_at: DateTime<Utc>,
) -> sqlx::Result<()> {
    let owner = sqlx::query_scalar::<_, Option<Uuid>>(
        "SELECT COALESCE(m.user_id, s.user_id) FROM sessions s \
         LEFT JOIN machines m ON m.id = s.machine_uuid WHERE s.id = $1",
    )
    .bind(draft_id)
    .fetch_optional(pool)
    .await?
    .flatten();
    let owner = owner.or(user_id).ok_or(sqlx::Error::RowNotFound)?;
    let lane_id = lane(pool, owner, draft_id).await?;
    sqlx::query(
        "INSERT INTO inbox_events \
           (lane_id, user_id, source, source_id, body, payload, not_before, next_attempt_at) \
         VALUES ($1, $2, 'timer', $3, $4, '{\"kind\": \"draft_launch\"}', $5, $5) \
         ON CONFLICT (lane_id, source, source_id) DO UPDATE \
         SET user_id = EXCLUDED.user_id, status = 'pending', not_before = $5, \
             next_attempt_at = $5, attempts = 0, last_error = NULL, batch_id = NULL, \
             delivered_at = NULL, delivered_session_id = NULL, delivered_as = NULL",
    )
    .bind(lane_id)
    .bind(user_id.unwrap_or(owner))
    .bind(draft_id)
    .bind(EVENT_BODY)
    .bind(launch_at)
    .execute(pool)
    .await?;
    announce(bus, draft_id, user_id, ScheduledLaunchState::Scheduled, Some(launch_at), None);
    Ok(())
}

/// Drop a draft's schedule, leaving the draft itself in place.
pub async fn cancel(pool: &sqlx::PgPool, bus: &Bus, draft_id: &str) -> sqlx::Result<bool> {
    let cancelled: Vec<Uuid> = sqlx::query_scalar(
        "UPDATE inbox_events e SET status = 'cancelled', last_error = 'cancelled by the user' \
         FROM inbox_lanes l \
         WHERE l.id = e.lane_id AND l.key = $1 AND e.source = 'timer' AND e.source_id = $2 \
           AND e.status IN ('pending', 'claimed', 'dead') \
         RETURNING e.user_id",
    )
    .bind(lane_key(draft_id))
    .bind(draft_id)
    .fetch_all(pool)
    .await?;
    let Some(user_id) = cancelled.first().copied() else { return Ok(false) };
    announce(bus, draft_id, Some(user_id), ScheduledLaunchState::Cancelled, None, None);
    Ok(true)
}

/// `(launch_at, last_error)` per scheduled draft, for the drafts list.
pub async fn pending_for(
    pool: &sqlx::PgPool,
    draft_ids: &[String],
) -> sqlx::Result<std::collections::HashMap<String, (DateTime<Utc>, Option<String>)>> {
    let rows: Vec<(String, DateTime<Utc>, Option<String>)> = sqlx::query_as(
        "SELECT e.source_id, e.not_before, e.last_error \
         FROM inbox_events e JOIN inbox_lanes l ON l.id = e.lane_id \
         WHERE e.source = 'timer' AND e.source_id = ANY($1) \
           AND l.key = 'draft:' || e.source_id \
           AND e.status IN ('pending', 'claimed', 'dead') \
           AND EXISTS (SELECT 1 FROM sessions s WHERE s.id = e.source_id)",
    )
    .bind(draft_ids)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(id, at, err)| (id, (at, err))).collect())
}

/// The lane worker's delivery. A discarded draft takes its schedule with it.
pub async fn launch(state: &AppState, draft_id: &str, events: &[BatchEvent]) -> Outcome {
    let Some(batch_id) = events.first().map(|e| e.batch_id) else { return Outcome::Skip };
    let facts = match draft_facts(&state.pool, draft_id).await {
        Ok(facts) => facts,
        Err(e) => return Outcome::Retry(e.to_string()),
    };
    let Some((status, owner)) = facts else {
        return match drop_batch(&state.pool, batch_id).await {
            Ok(()) => Outcome::Skip,
            Err(e) => Outcome::Retry(e.to_string()),
        };
    };
    if let Some(reason) = unlaunchable_reason(Some(&status)) {
        return Outcome::Dead(reason.to_owned());
    }
    let Some(owner) = owner else {
        return Outcome::Dead("draft's machine no longer exists".to_owned());
    };
    let ctx = AuthContext {
        user_id: owner,
        key_id: Uuid::nil(),
        machine_id: None,
        scopes: [Scope::Read, Scope::Dispatch].into_iter().collect(),
    };
    match crate::routes::spawn::launch_stored_draft(
        state,
        &ctx,
        draft_id,
        std::collections::BTreeMap::new(),
    )
    .await
    {
        Ok((_, axum::Json(launched))) => {
            tracing::info!(draft = %draft_id, "scheduled draft launched");
            Outcome::Delivered {
                session_id: launched
                    .session_id
                    .map_or_else(|| launched.command_id.to_string(), |id| id.to_string()),
                delivered_as: "spawn",
            }
        }
        Err(e) => Outcome::Retry(e.to_string()),
    }
}

async fn drop_batch(pool: &sqlx::PgPool, batch_id: Uuid) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE inbox_events SET status = 'cancelled', last_error = $2 \
         WHERE batch_id = $1 AND status = 'claimed'",
    )
    .bind(batch_id)
    .bind(GONE)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Announce where a lane worker's attempt left the draft's schedule.
pub async fn announce_settled(state: &AppState, lane_id: Uuid, draft_id: &str) -> sqlx::Result<()> {
    let settled: Option<(String, Uuid, Option<String>)> = sqlx::query_as(
        "SELECT status, user_id, last_error FROM inbox_events \
         WHERE lane_id = $1 AND source = 'timer' AND source_id = $2",
    )
    .bind(lane_id)
    .bind(draft_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((status, user_id, last_error)) = settled else { return Ok(()) };
    let launch_state = match (status.as_str(), last_error.is_some()) {
        ("delivered", _) => ScheduledLaunchState::Launched,
        ("dead", _) => ScheduledLaunchState::Dead,
        ("pending", true) => ScheduledLaunchState::Failed,
        _ => return Ok(()),
    };
    announce(&state.bus, draft_id, Some(user_id), launch_state, None, last_error.as_deref());
    Ok(())
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
            if let ServerEvent::ScheduledLaunch {
                draft_id: id,
                user_id,
                state,
                launch_at,
                last_error,
            } = &*frame.event
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

    async fn lane_of(pool: &sqlx::PgPool, draft: &str) -> Uuid {
        sqlx::query_scalar("SELECT id FROM inbox_lanes WHERE key = $1")
            .bind(lane_key(draft))
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn event_of(pool: &sqlx::PgPool, draft: &str) -> (String, i32, Option<String>) {
        sqlx::query_as(
            "SELECT e.status, e.attempts, e.last_error FROM inbox_events e \
             JOIN inbox_lanes l ON l.id = e.lane_id \
             WHERE l.key = $1 AND e.source = 'timer' AND e.source_id = $2",
        )
        .bind(lane_key(draft))
        .bind(draft)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    async fn claim(pool: &sqlx::PgPool, draft: &str) -> Vec<BatchEvent> {
        let lane = lane_of(pool, draft).await;
        let filter = crate::inbox::claim::LaneFilter { lane: Some(lane), machine: None };
        let lanes = crate::inbox::claim::claim_lanes(pool, 1, filter).await.unwrap();
        if lanes.is_empty() {
            return Vec::new();
        }
        let events = crate::inbox::claim::claim_batch(pool, lane).await.unwrap();
        crate::inbox::claim::release_lane(pool, lane).await.unwrap();
        events
    }

    #[tokio::test]
    async fn a_draft_schedule_is_one_timer_event_claimed_once_then_backs_off_and_dead_letters() {
        let name = "scheduled_spawn_claim";
        let Some(url) = crate::routes::gateway::test_db_url(name) else { return };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .connect(&url)
            .await
            .expect("connect test db");
        let (due, owner) = seed_owned_draft(&pool, name).await;
        let later = seed_draft(&pool, name).await;
        let bus = test_bus();
        schedule(&pool, &bus, &due, None, Utc::now() + chrono::Duration::hours(5)).await.unwrap();
        schedule(&pool, &bus, &due, None, now()).await.unwrap();
        schedule(&pool, &bus, &later, None, Utc::now() + chrono::Duration::hours(1)).await.unwrap();
        let lanes: (Uuid, String) =
            sqlx::query_as("SELECT user_id, key FROM inbox_lanes WHERE id = $1")
                .bind(lane_of(&pool, &due).await)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(lanes, (owner, format!("draft:{due}")), "the lane belongs to the machine owner");
        assert_eq!(event_of(&pool, &due).await, ("pending".to_owned(), 0, None));

        let (a, b) = tokio::join!(claim(&pool, &due), claim(&pool, &due));
        assert_eq!(a.len() + b.len(), 1, "a reschedule replaced the event; it is claimed once");
        assert!(claim(&pool, &later).await.is_empty(), "the future draft is not due");
        let batch = a.iter().chain(&b).map(|e| e.batch_id).next().unwrap();

        crate::inbox::claim::record_failure(&pool, batch, 0, "machine offline").await.unwrap();
        assert_eq!(
            event_of(&pool, &due).await,
            ("pending".to_owned(), 1, Some("machine offline".to_owned()))
        );
        assert!(claim(&pool, &due).await.is_empty(), "backing off");

        sqlx::query(
            "UPDATE inbox_events SET next_attempt_at = now() \
             WHERE source = 'timer' AND source_id = $1",
        )
        .bind(&due)
        .execute(&pool)
        .await
        .unwrap();
        let batch = claim(&pool, &due).await.first().map(|e| e.batch_id).expect("due again");
        crate::inbox::claim::record_failure(&pool, batch, 10, "still offline").await.unwrap();
        let dead = event_of(&pool, &due).await.0;
        assert_eq!(dead, "dead", "a dead-lettered schedule keeps its draft");
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
        assert!(pending_for(&pool, std::slice::from_ref(&later)).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_timed_draft_launches_with_its_sealed_env() {
        let name = "scheduled_spawn_sealed_env";
        let Some(url) = crate::routes::gateway::test_db_url(name) else { return };
        crate::crypto::install_vault_key(vec![7u8; 32]);
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .connect(&url)
            .await
            .expect("connect test db");
        let (_, owner) = seed_owned_draft(&pool, name).await;
        let machine: Uuid = sqlx::query_scalar("SELECT id FROM machines WHERE user_id = $1")
            .bind(owner)
            .fetch_one(&pool)
            .await
            .unwrap();
        let state = AppState::for_test(pool.clone());
        let req: cctui_proto::api::SpawnRequest = serde_json::from_value(serde_json::json!({
            "machine_id": machine.to_string(), "working_dir": "/w", "prompt": "digest",
            "permission_mode": "yolo", "no_account": true, "save_draft": true,
            "env": {"FOO": "bar-sealed-value"},
        }))
        .unwrap();
        let draft = crate::routes::spawn::store_spawn_row(
            &state,
            &req,
            machine,
            &[],
            SessionRowStatus::Draft,
        )
        .await
        .unwrap()
        .to_string();
        let metadata: serde_json::Value =
            sqlx::query_scalar("SELECT metadata FROM sessions WHERE id = $1")
                .bind(&draft)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(!metadata.to_string().contains("bar-sealed-value"), "{metadata}");
        assert_eq!(metadata["draft"]["env_keys"], serde_json::json!(["FOO"]));

        schedule(&pool, &state.bus, &draft, Some(owner), now()).await.unwrap();
        let events = claim(&pool, &draft).await;
        assert_eq!(events.len(), 1);
        let daemon = crate::routes::fs::answer_list_dirs(&state, machine, Ok(Vec::new()));
        let outcome = launch(&state, &draft, &events).await;
        let mut rx = daemon.await.unwrap();
        assert!(matches!(outcome, Outcome::Delivered { .. }), "{outcome:?}");
        let Some(cctui_proto::ws::DaemonFrameDown::Command { command, .. }) = rx.recv().await
        else {
            panic!("expected a command frame")
        };
        let cctui_proto::adapter::AdapterCommand::Spawn { spec, .. } = *command else {
            panic!("expected a spawn")
        };
        assert_eq!(spec.env.get("FOO").map(String::as_str), Some("bar-sealed-value"));
    }

    async fn auto_archive_intents(pool: &sqlx::PgPool, spawn_key: &str) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM session_auto_archive WHERE spawn_key = $1")
            .bind(spawn_key)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn launch_scheduled_draft(
        state: &AppState,
        owner: Uuid,
        machine: Uuid,
        auto_archive: bool,
    ) -> String {
        let req: cctui_proto::api::SpawnRequest = serde_json::from_value(serde_json::json!({
            "machine_id": machine.to_string(), "working_dir": "/w", "prompt": "nightly",
            "no_account": true, "save_draft": true, "auto_archive": auto_archive,
        }))
        .unwrap();
        let draft = crate::routes::spawn::store_spawn_row(
            state,
            &req,
            machine,
            &[],
            SessionRowStatus::Draft,
        )
        .await
        .unwrap()
        .to_string();
        schedule(&state.pool, &state.bus, &draft, Some(owner), now()).await.unwrap();
        let events = claim(&state.pool, &draft).await;
        assert_eq!(events.len(), 1);
        let daemon = crate::routes::fs::answer_list_dirs(state, machine, Ok(Vec::new()));
        let outcome = launch(state, &draft, &events).await;
        drop(daemon.await.unwrap());
        let Outcome::Delivered { session_id, .. } = outcome else {
            panic!("expected a delivered launch, got {outcome:?}")
        };
        session_id
    }

    #[tokio::test]
    async fn a_scheduled_draft_keeps_its_auto_archive_intent() {
        let name = "scheduled_spawn_auto_archive";
        let Some(url) = crate::routes::gateway::test_db_url(name) else { return };
        crate::crypto::install_vault_key(vec![7u8; 32]);
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .connect(&url)
            .await
            .expect("connect test db");
        let (_, owner) = seed_owned_draft(&pool, name).await;
        let machine: Uuid = sqlx::query_scalar("SELECT id FROM machines WHERE user_id = $1")
            .bind(owner)
            .fetch_one(&pool)
            .await
            .unwrap();
        let state = AppState::for_test(pool.clone());

        let armed = launch_scheduled_draft(&state, owner, machine, true).await;
        assert_eq!(auto_archive_intents(&pool, &armed).await, 1, "a ticked draft arms it");
        let plain = launch_scheduled_draft(&state, owner, machine, false).await;
        assert_eq!(auto_archive_intents(&pool, &plain).await, 0, "an unticked draft does not");

        sqlx::query("DELETE FROM session_auto_archive WHERE spawn_key = $1")
            .bind(&armed)
            .execute(&pool)
            .await
            .ok();
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
        schedule(&pool, &test_bus(), &draft, None, now()).await.unwrap();
        sqlx::query("DELETE FROM sessions WHERE id = $1")
            .bind(&draft)
            .execute(&pool)
            .await
            .unwrap();
        assert!(pending_for(&pool, std::slice::from_ref(&draft)).await.unwrap().is_empty());

        let state = AppState::for_test(pool.clone());
        let events = claim(&pool, &draft).await;
        assert_eq!(events.len(), 1);
        assert_eq!(launch(&state, &draft, &events).await, Outcome::Skip);
        let (status, _, err) = event_of(&pool, &draft).await;
        assert_eq!((status.as_str(), err.as_deref()), ("cancelled", Some(GONE)), "not dead");
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
        let state = AppState::for_test(pool.clone());
        let bus = &state.bus;
        let mut rx = bus.subscribe_server();
        let (draft, owner) = seed_owned_draft(&pool, name).await;
        let at = Utc::now() + chrono::Duration::hours(3);

        schedule(&pool, bus, &draft, Some(owner), at).await.unwrap();
        let (st, user, launch_at, err) = next_launch_event(&mut rx, &draft).expect("scheduled");
        assert_eq!((st, user, err), (ScheduledLaunchState::Scheduled, Some(owner), None));
        assert_eq!(launch_at, Some(at), "the schedule rides along so no refetch is needed");

        assert!(cancel(&pool, bus, &draft).await.unwrap());
        let (st, user, ..) = next_launch_event(&mut rx, &draft).expect("cancelled");
        assert_eq!((st, user), (ScheduledLaunchState::Cancelled, Some(owner)));
        assert!(
            next_launch_event(&mut rx, &draft).is_none(),
            "cancelling an unscheduled draft is silent"
        );
        assert!(!cancel(&pool, bus, &draft).await.unwrap());
        assert!(next_launch_event(&mut rx, &draft).is_none());

        schedule(&pool, bus, &draft, Some(owner), at).await.unwrap();
        next_launch_event(&mut rx, &draft).expect("rescheduled");
        let lane = lane_of(&pool, &draft).await;

        for (status, error, expected) in [
            ("pending", Some("machine offline"), ScheduledLaunchState::Failed),
            ("dead", Some("still offline"), ScheduledLaunchState::Dead),
            ("delivered", None, ScheduledLaunchState::Launched),
        ] {
            sqlx::query(
                "UPDATE inbox_events SET status = $3, last_error = $4 \
                 WHERE lane_id = $1 AND source = 'timer' AND source_id = $2",
            )
            .bind(lane)
            .bind(&draft)
            .bind(status)
            .bind(error)
            .execute(&pool)
            .await
            .unwrap();
            announce_settled(&state, lane, &draft).await.unwrap();
            let (st, user, _, err) = next_launch_event(&mut rx, &draft).expect(status);
            assert_eq!((st, user, err.as_deref()), (expected, Some(owner), error));
        }

        sqlx::query(
            "UPDATE inbox_events SET status = 'pending', last_error = NULL WHERE lane_id = $1",
        )
        .bind(lane)
        .execute(&pool)
        .await
        .unwrap();
        announce_settled(&state, lane, &draft).await.unwrap();
        assert!(next_launch_event(&mut rx, &draft).is_none(), "a deferred launch is silent");
    }
}
