use sqlx::PgPool;
use uuid::Uuid;

use super::Source;
use super::claim::{
    LaneFilter, claim_batch, claim_lanes, reclaim_stranded, record_failure, release_lane,
};
use super::store::{NewEvent, insert_event, session_lane};
use super::store_tests::seed_session;
use crate::outbox::{MAX_ATTEMPTS, Retry};

async fn pool(name: &str) -> Option<PgPool> {
    let url = crate::routes::gateway::test_db_url(name)?;
    Some(
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .connect(&url)
            .await
            .expect("connect test db"),
    )
}

async fn lane_with_events(pool: &PgPool, name: &str, bodies: &[&str]) -> (Uuid, String) {
    let (uid, sid) = seed_session(pool, name).await;
    let lane = session_lane(pool, uid, &sid).await.unwrap();
    for body in bodies {
        insert_event(
            pool,
            &NewEvent {
                lane_id: lane,
                user_id: uid,
                source: Source::N8n,
                source_id: Some(&Uuid::new_v4().to_string()),
                body,
                payload: None,
                not_before: None,
                expires_at: None,
            },
        )
        .await
        .unwrap();
    }
    (lane, sid)
}

const fn only(lane: Uuid) -> LaneFilter {
    LaneFilter { lane: Some(lane), machine: None }
}

async fn claimable(pool: &PgPool, lane: Uuid) -> bool {
    let claimed = claim_lanes(pool, 10, only(lane)).await.unwrap();
    if claimed.is_empty() {
        return false;
    }
    release_lane(pool, lane).await.unwrap();
    true
}

async fn exec(pool: &PgPool, sql: &str, id: &str) {
    sqlx::query(sql).bind(id).execute(pool).await.unwrap();
}

async fn event_rows(pool: &PgPool, lane: Uuid) -> Vec<(String, i32, Option<Uuid>)> {
    sqlx::query_as(
        "SELECT status, attempts, batch_id FROM inbox_events WHERE lane_id = $1 ORDER BY seq",
    )
    .bind(lane)
    .fetch_all(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn two_concurrent_lane_claims_have_one_winner() {
    let Some(pool) = pool("inbox_claim_race").await else { return };
    let (lane, _) = lane_with_events(&pool, "claim-race", &["one"]).await;
    let (a, b) =
        tokio::join!(claim_lanes(&pool, 10, only(lane)), claim_lanes(&pool, 10, only(lane)));
    let won = a.unwrap().len() + b.unwrap().len();
    assert_eq!(won, 1, "exactly one replica holds the lane");
    assert!(claim_lanes(&pool, 10, only(lane)).await.unwrap().is_empty(), "lease still held");
    release_lane(&pool, lane).await.unwrap();
    assert!(claimable(&pool, lane).await, "released lanes are claimable again");
}

#[tokio::test]
async fn mid_turn_blocked_limited_and_paused_lanes_are_not_claimable() {
    let Some(pool) = pool("inbox_not_claimable").await else { return };
    let (lane, sid) = lane_with_events(&pool, "not-claimable", &["one"]).await;
    let lane_id = lane.to_string();
    assert!(claimable(&pool, lane).await);
    let blockers = [
        ("UPDATE sessions SET tempo = 'active' WHERE id = $1", "mid-turn tempo"),
        ("UPDATE sessions SET agent_state = 'working' WHERE id = $1", "working"),
        ("UPDATE sessions SET agent_state = 'running' WHERE id = $1", "running"),
        ("UPDATE sessions SET tempo = 'blocked' WHERE id = $1", "blocked"),
        ("UPDATE sessions SET soft_limit_reason = 'usage' WHERE id = $1", "soft limited"),
    ];
    let reset = "UPDATE sessions SET tempo = NULL, agent_state = NULL, soft_limit_reason = NULL \
                 WHERE id = $1";
    for (sql, why) in blockers {
        exec(&pool, sql, &sid).await;
        assert!(!claimable(&pool, lane).await, "{why} must not be claimable");
        exec(&pool, reset, &sid).await;
    }
    exec(&pool, "UPDATE inbox_lanes SET paused = true WHERE id = $1::uuid", &lane_id).await;
    assert!(!claimable(&pool, lane).await, "paused must not be claimable");
    exec(&pool, "UPDATE inbox_lanes SET paused = false WHERE id = $1::uuid", &lane_id).await;
    assert!(claimable(&pool, lane).await);
    let pending: Vec<String> =
        event_rows(&pool, lane).await.into_iter().map(|(status, ..)| status).collect();
    assert_eq!(pending, ["pending"], "a skipped lane costs its events nothing");
}

#[tokio::test]
async fn a_queued_spawn_is_claimable_only_while_its_machine_is_reachable() {
    let Some(pool) = pool("inbox_queued_claimable").await else { return };
    let (lane, sid) = lane_with_events(&pool, "queued-claim", &["launch"]).await;
    exec(&pool, "UPDATE sessions SET status = 'queued' WHERE id = $1", &sid).await;
    let machine: Uuid = sqlx::query_scalar("SELECT machine_uuid FROM sessions WHERE id = $1")
        .bind(&sid)
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE machines SET last_seen_at = now() - interval '2 hours' WHERE id = $1")
        .bind(machine)
        .execute(&pool)
        .await
        .unwrap();
    assert!(!claimable(&pool, lane).await, "offline machine");

    let woke = LaneFilter { lane: Some(lane), machine: Some(machine) };
    assert_eq!(claim_lanes(&pool, 10, woke).await.unwrap().len(), 1, "daemon just connected");
    release_lane(&pool, lane).await.unwrap();
    let other = LaneFilter { lane: Some(lane), machine: Some(Uuid::new_v4()) };
    assert!(claim_lanes(&pool, 10, other).await.unwrap().is_empty(), "another machine woke");

    sqlx::query("UPDATE machines SET last_seen_at = now() WHERE id = $1")
        .bind(machine)
        .execute(&pool)
        .await
        .unwrap();
    assert!(claimable(&pool, lane).await, "heartbeat inside the online window");
}

#[tokio::test]
async fn three_due_events_batch_into_one_marked_turn() {
    let Some(pool) = pool("inbox_batch_three").await else { return };
    let (lane, _) = lane_with_events(&pool, "batch", &["first", "second", "third"]).await;
    let claimed = claim_lanes(&pool, 10, only(lane)).await.unwrap();
    assert_eq!(claimed.len(), 1);
    let events = claim_batch(&pool, lane).await.unwrap();
    assert_eq!(events.len(), 3);
    assert!(events.iter().all(|e| e.batch_id == events[0].batch_id), "one batch, one fate");
    let text = super::deliver::batch_text(&events, Vec::new).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "[cctui inbox: 3 events]");
    for (line, (n, body)) in lines[1..].iter().zip([(1, "first"), (2, "second"), (3, "third")]) {
        assert!(line.starts_with(&format!("{n}. (n8n, ")), "{line}");
        assert!(line.ends_with(&format!(") {body}")), "{line}");
    }
    let rebatched = claim_batch(&pool, lane).await.unwrap();
    assert!(rebatched.is_empty(), "claimed events are not batched twice");
}

#[tokio::test]
async fn a_failing_batch_backs_off_then_dead_letters() {
    let Some(pool) = pool("inbox_retry_dead").await else { return };
    let (lane, _) = lane_with_events(&pool, "retry", &["flaky"]).await;
    let batch = claim_batch(&pool, lane).await.unwrap()[0].batch_id;
    let retry = record_failure(&pool, batch, 0, "daemon offline").await.unwrap();
    assert!(matches!(retry, Retry::After { attempts: 1, .. }), "{retry:?}");
    assert_eq!(event_rows(&pool, lane).await, [("pending".to_owned(), 1, None)]);
    assert!(claim_batch(&pool, lane).await.unwrap().is_empty(), "backing off");

    sqlx::query("UPDATE inbox_events SET next_attempt_at = now(), attempts = $2 WHERE lane_id = $1")
        .bind(lane)
        .bind(MAX_ATTEMPTS - 1)
        .execute(&pool)
        .await
        .unwrap();
    let events = claim_batch(&pool, lane).await.unwrap();
    let retry =
        record_failure(&pool, events[0].batch_id, events[0].attempts, "daemon offline").await;
    assert_eq!(retry.unwrap(), Retry::Dead { attempts: MAX_ATTEMPTS });
    let rows = event_rows(&pool, lane).await;
    assert_eq!((rows[0].0.as_str(), rows[0].1), ("dead", MAX_ATTEMPTS));
}

#[tokio::test]
async fn a_stranded_batch_is_retaken_whole_under_its_batch_id() {
    let Some(pool) = pool("inbox_stranded").await else { return };
    let (lane, _) = lane_with_events(&pool, "stranded", &["a", "b"]).await;
    let first = claim_batch(&pool, lane).await.unwrap();
    sqlx::query(
        "UPDATE inbox_events SET next_attempt_at = now() - interval '1 second' \
         WHERE lane_id = $1",
    )
    .bind(lane)
    .execute(&pool)
    .await
    .unwrap();
    assert!(reclaim_stranded(&pool).await.unwrap() >= 2);
    let owner: Uuid = sqlx::query_scalar("SELECT user_id FROM inbox_lanes WHERE id = $1")
        .bind(lane)
        .fetch_one(&pool)
        .await
        .unwrap();
    insert_event(
        &pool,
        &NewEvent {
            lane_id: lane,
            user_id: owner,
            source: Source::Timer,
            source_id: None,
            body: "late",
            payload: None,
            not_before: None,
            expires_at: None,
        },
    )
    .await
    .unwrap();
    let again = claim_batch(&pool, lane).await.unwrap();
    assert_eq!(again.len(), 2, "the newcomer waits for the next batch");
    assert!(again.iter().all(|e| e.batch_id == first[0].batch_id));
}

#[tokio::test]
async fn a_queued_spawn_past_a_day_fails_and_its_events_cancel() {
    let Some(pool) = pool("inbox_queued_expiry").await else { return };
    let (old_lane, old) = lane_with_events(&pool, "queued-old", &[]).await;
    let (fresh_lane, fresh) = lane_with_events(&pool, "queued-fresh", &[]).await;
    for sid in [&old, &fresh] {
        exec(&pool, "UPDATE sessions SET status = 'queued' WHERE id = $1", sid).await;
        let owner: Uuid = sqlx::query_scalar("SELECT user_id FROM sessions WHERE id = $1")
            .bind(sid)
            .fetch_one(&pool)
            .await
            .unwrap();
        super::queued::enqueue(&pool, owner, owner, sid).await.unwrap();
    }
    exec(
        &pool,
        "UPDATE sessions SET registered_at = now() - interval '25 hours' WHERE id = $1",
        &old,
    )
    .await;

    let expired = super::queued::expire(&pool).await.unwrap();
    assert!(expired.contains(&old) && !expired.contains(&fresh), "{expired:?}");
    let (status, reason, detail): (String, Option<String>, Option<String>) =
        sqlx::query_as("SELECT status, end_reason, end_detail FROM sessions WHERE id = $1")
            .bind(&old)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "failed");
    assert_eq!(reason.as_deref(), Some("spawn_failed"));
    assert_eq!(detail.as_deref(), Some(super::queued::EXPIRED));
    assert_eq!(event_rows(&pool, old_lane).await[0].0, "cancelled");
    assert_eq!(event_rows(&pool, fresh_lane).await[0].0, "pending");
}

#[tokio::test]
async fn cancelling_touches_only_a_queued_spawns_events() {
    let Some(pool) = pool("inbox_queued_cancel").await else { return };
    let (live_lane, live) = lane_with_events(&pool, "queued-live", &["reply"]).await;
    assert_eq!(super::queued::cancel(&pool, &live).await.unwrap(), 0);
    assert_eq!(event_rows(&pool, live_lane).await[0].0, "pending");

    exec(&pool, "UPDATE sessions SET status = 'queued' WHERE id = $1", &live).await;
    assert_eq!(super::queued::cancel(&pool, &live).await.unwrap(), 1);
    assert_eq!(event_rows(&pool, live_lane).await[0].0, "cancelled");
}
