use chrono::{Duration, Utc};
use sqlx::PgPool;

use super::{Accepted, TimerRequest, archive, schedule};
use crate::state::AppState;
use cctui_server_kernel::test_support::seed_session;

fn request(delay_secs: u32) -> TimerRequest {
    TimerRequest { delay_secs, prompt: " check the CI run ".into(), tool_use_id: "toolu_1".into() }
}

async fn state(name: &str) -> Option<AppState> {
    let url = cctui_server_kernel::test_support::db_url(name)?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .expect("connect test db");
    Some(AppState::for_test(pool))
}

async fn timer_events(pool: &PgPool, sid: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM inbox_events WHERE source = 'timer' AND parent_session_id = $1",
    )
    .bind(sid)
    .fetch_one(pool)
    .await
    .unwrap()
}

type EventRow = (String, String, String, chrono::DateTime<Utc>, String);

#[tokio::test]
async fn a_timer_under_the_ttl_arms_a_tick_the_sweep_claims_once() {
    let Some(state) = state("a_timer_under_the_ttl_arms_a_tick").await else { return };
    let pool = &state.pool;
    let (_, sid) = seed_session(pool, "timer-tick").await;
    let now = Utc::now();
    let accepted = schedule(&state, &sid, &request(600), now).await.unwrap().unwrap();
    assert_eq!(accepted, Accepted::Tick { fires_at: now + Duration::seconds(600) });
    assert_eq!(timer_events(pool, &sid).await, 0, "a tick writes no inbox event");

    sqlx::query(
        "UPDATE session_timers SET timer_at = now() - interval '1 second' \
         WHERE session_id = $1",
    )
    .bind(&sid)
    .execute(pool)
    .await
    .unwrap();
    let claimed = crate::keepalive::claim_timers(pool).await.unwrap();
    let mine: Vec<_> = claimed.iter().filter(|t| t.id == sid).collect();
    assert_eq!(mine.len(), 1);
    assert_eq!((mine[0].prompt.as_str(), mine[0].timer_ticks), ("check the CI run", 1));
    let again = crate::keepalive::claim_timers(pool).await.unwrap();
    assert!(again.iter().all(|t| t.id != sid), "a claimed timer is gone");
}

#[tokio::test]
async fn the_timer_budget_refuses_a_wake_past_the_tick_budget() {
    let Some(state) = state("the_timer_budget_refuses_a_wake").await else { return };
    let (_, sid) = seed_session(&state.pool, "timer-budget").await;
    sqlx::query("INSERT INTO session_timers (session_id, timer_ticks) VALUES ($1, 6)")
        .bind(&sid)
        .execute(&state.pool)
        .await
        .unwrap();
    let refused = schedule(&state, &sid, &request(600), Utc::now()).await.unwrap();
    assert!(refused.unwrap_err().contains("budget"));
}

#[tokio::test]
async fn a_timer_over_the_ttl_queues_an_event_and_archives_the_session() {
    let Some(state) = state("a_timer_over_the_ttl_queues_an_event").await else { return };
    let pool = &state.pool;
    let (_, sid) = seed_session(pool, "timer-event").await;
    let now = Utc::now();
    let Accepted::Event { fires_at, event_id, deduplicated, archives } =
        schedule(&state, &sid, &request(7200), now).await.unwrap().unwrap()
    else {
        panic!("over the TTL is an inbox event");
    };
    assert!(!deduplicated && archives);
    assert_eq!(fires_at, now + Duration::seconds(7200));
    let (source_id, body, status, not_before, lane_key): EventRow = sqlx::query_as(
        "SELECT e.source_id, e.body, e.status, e.not_before, l.key \
             FROM inbox_events e JOIN inbox_lanes l ON l.id = e.lane_id WHERE e.id = $1",
    )
    .bind(event_id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(source_id, format!("{sid}:toolu_1"));
    assert_eq!((body.as_str(), status.as_str()), ("check the CI run", "pending"));
    assert_eq!(lane_key, format!("session:{sid}"));
    assert!((not_before - fires_at).num_milliseconds().abs() < 1000);

    let retry = schedule(&state, &sid, &request(7200), now).await.unwrap().unwrap();
    assert!(matches!(retry, Accepted::Event { deduplicated: true, archives: false, .. }));
    assert_eq!(timer_events(pool, &sid).await, 1);

    assert!(archive(&state, &sid).await.unwrap());
    let status: String = sqlx::query_scalar("SELECT status FROM sessions WHERE id = $1")
        .bind(&sid)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(status, "archived");
}
