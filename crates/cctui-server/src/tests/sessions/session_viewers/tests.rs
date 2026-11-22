use super::*;

#[tokio::test]
async fn a_viewer_counts_until_its_socket_leaves_or_its_stamp_lapses() {
    let Some(url) = cctui_server_kernel::test_support::db_url("session_viewers") else {
        return;
    };
    let pool = PgPool::connect(&url).await.expect("connect test db");
    let state = AppState::for_test(pool.clone());
    let sid = uuid::Uuid::new_v4().to_string();
    let ids = [sid.clone()];

    assert!(!any_viewed(&pool, &ids).await.unwrap());
    assert!(state.bus.view_watch_inc(&sid));
    opened(&state, &sid).await;
    assert!(any_viewed(&pool, &ids).await.unwrap(), "an open session is viewed");

    heartbeat(&state).await;
    assert!(any_viewed(&pool, &ids).await.unwrap(), "the heartbeat keeps a held view");

    sqlx::query(
        "UPDATE session_viewers SET seen_at = now() - interval '1 minute' \
         WHERE session_id = $1",
    )
    .bind(&sid)
    .execute(&pool)
    .await
    .unwrap();
    assert!(!any_viewed(&pool, &ids).await.unwrap(), "a lapsed stamp is no viewer");
    heartbeat(&state).await;
    assert!(any_viewed(&pool, &ids).await.unwrap(), "the heartbeat re-stamps it");

    assert!(state.bus.view_watch_dec(&sid));
    closed(&state, &sid).await;
    assert!(!any_viewed(&pool, &ids).await.unwrap(), "the viewer left");

    sqlx::query("INSERT INTO session_viewers (session_id, pod) VALUES ($1, $2)")
        .bind(&sid)
        .bind(&state.presence.pod)
        .execute(&pool)
        .await
        .unwrap();
    heartbeat(&state).await;
    assert!(!any_viewed(&pool, &ids).await.unwrap(), "a row this pod no longer holds goes");
}
