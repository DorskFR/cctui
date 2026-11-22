use sqlx::PgPool;
use uuid::Uuid;

use super::guards::set_paused;
use super::store::session_lane;
use super::store_tests::seed_session;

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

async fn paused(pool: &PgPool, lane: Uuid) -> (bool, Option<String>) {
    sqlx::query_as("SELECT paused, paused_reason FROM inbox_lanes WHERE id = $1")
        .bind(lane)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_manual_pause_is_owner_scoped() {
    let Some(pool) = pool("inbox_guard_manual_pause").await else { return };
    let (uid, sid) = seed_session(&pool, "manual-pause").await;
    let (stranger, _) = seed_session(&pool, "manual-pause").await;
    let lane = session_lane(&pool, uid, &sid).await.unwrap();
    assert!(set_paused(&pool, Some(stranger), lane, true).await.unwrap().is_none());
    assert_eq!(paused(&pool, lane).await, (false, None));
    let mine = set_paused(&pool, Some(uid), lane, true).await.unwrap().unwrap();
    assert_eq!((mine.paused, mine.paused_reason.as_deref()), (true, Some("manual")));
    let admin = set_paused(&pool, None, lane, false).await.unwrap().unwrap();
    assert!(!admin.paused, "the admin filter sees every lane");
}
