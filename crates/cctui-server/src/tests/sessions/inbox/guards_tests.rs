use sqlx::PgPool;
use uuid::Uuid;

use super::Source;
use super::guards::{CEILING, EVENTS_PER_HOUR, flag_stuck, set_paused, trip_ceiling};
use super::store::{NewEvent, insert_event, session_lane};
use cctui_server_kernel::test_support::seed_session;
use cctui_server_notify::notify::kind;

async fn pool(name: &str) -> Option<PgPool> {
    let url = cctui_server_kernel::test_support::db_url(name)?;
    Some(
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .connect(&url)
            .await
            .expect("connect test db"),
    )
}

async fn add_events(pool: &PgPool, lane: Uuid, user: Uuid, n: i64) {
    for _ in 0..n {
        insert_event(
            pool,
            &NewEvent {
                lane_id: lane,
                user_id: user,
                source: Source::Timer,
                source_id: Some(&Uuid::new_v4().to_string()),
                body: "tick",
                payload: None,
                not_before: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
                expires_at: None,
            },
        )
        .await
        .unwrap();
    }
}

async fn paused(pool: &PgPool, lane: Uuid) -> (bool, Option<String>) {
    sqlx::query_as("SELECT paused, paused_reason FROM inbox_lanes WHERE id = $1")
        .bind(lane)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn the_hourly_ceiling_pauses_the_lane_once_and_resume_restarts_the_window() {
    let Some(pool) = pool("inbox_guard_ceiling").await else { return };
    let (uid, sid) = seed_session(&pool, "ceiling").await;
    let lane = session_lane(&pool, uid, &sid).await.unwrap();
    add_events(&pool, lane, uid, EVENTS_PER_HOUR).await;
    assert!(trip_ceiling(&pool, lane).await.unwrap().is_none(), "at the ceiling, not over it");

    add_events(&pool, lane, uid, 1).await;
    let (a, b) = tokio::join!(trip_ceiling(&pool, lane), trip_ceiling(&pool, lane));
    let notices: Vec<_> = [a.unwrap(), b.unwrap()].into_iter().flatten().collect();
    assert_eq!(notices.len(), 1, "exactly one call pauses the lane and notifies");
    let notice = &notices[0];
    assert_eq!((notice.kind, notice.user_id), (kind::INBOX_STUCK, uid));
    assert_eq!(notice.session_id.as_deref(), Some(sid.as_str()));
    assert_eq!(notice.subject_id, Some(lane.to_string()));
    assert_eq!(notice.collapse_key, Some(format!("inbox_ceiling:{lane}")));
    assert!(notice.body.as_deref().unwrap().contains(&format!("/inbox/lanes/{lane}/resume")));
    assert_eq!(paused(&pool, lane).await, (true, Some(CEILING.to_owned())));
    assert!(trip_ceiling(&pool, lane).await.unwrap().is_none(), "already paused");

    set_paused(&pool, Some(uid), lane, false).await.unwrap().expect("own lane");
    assert_eq!(paused(&pool, lane).await, (false, None));
    assert!(
        trip_ceiling(&pool, lane).await.unwrap().is_none(),
        "the burst before resume no longer counts"
    );
}

async fn add_overdue(pool: &PgPool, lane: Uuid, user: Uuid, minutes: i64) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO inbox_events (lane_id, user_id, source, body, not_before, next_attempt_at) \
         VALUES ($1, $2, 'timer', 'waiting', now() - $3::bigint * interval '1 minute', \
                 now() + interval '1 hour') \
         RETURNING id",
    )
    .bind(lane)
    .bind(user)
    .bind(minutes)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn a_stuck_lane_notifies_once_per_spell() {
    let Some(pool) = pool("inbox_guard_stuck").await else { return };
    let (uid, sid) = seed_session(&pool, "stuck").await;
    let lane = session_lane(&pool, uid, &sid).await.unwrap();
    add_overdue(&pool, lane, uid, 5).await;
    assert!(flag_stuck(&pool, Some(lane)).await.unwrap().is_empty(), "five minutes is not stuck");

    let old = add_overdue(&pool, lane, uid, 45).await;
    let notices = flag_stuck(&pool, Some(lane)).await.unwrap();
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].collapse_key, Some(format!("inbox_stuck:{lane}")));
    assert_eq!(notices[0].kind, kind::INBOX_STUCK);
    assert!(flag_stuck(&pool, Some(lane)).await.unwrap().is_empty(), "already notified");

    sqlx::query("UPDATE inbox_events SET status = 'cancelled' WHERE id = $1")
        .bind(old)
        .execute(&pool)
        .await
        .unwrap();
    assert!(flag_stuck(&pool, Some(lane)).await.unwrap().is_empty(), "drained: re-armed");
    add_overdue(&pool, lane, uid, 60).await;
    assert_eq!(flag_stuck(&pool, Some(lane)).await.unwrap().len(), 1, "a new spell notifies");

    set_paused(&pool, Some(uid), lane, false).await.unwrap();
    set_paused(&pool, Some(uid), lane, true).await.unwrap();
    assert!(flag_stuck(&pool, Some(lane)).await.unwrap().is_empty(), "a paused lane is quiet");
}
