use sqlx::PgPool;
use uuid::Uuid;

pub use cctui_server_kernel::test_support::seed_session;

use super::Source;
use super::store::{
    Cancel, NewEvent, Retry, cancel, get, insert_event, list, prune_delivered, retry, session_lane,
};

async fn pool(name: &str) -> Option<PgPool> {
    let url = cctui_server_kernel::test_support::db_url(name)?;
    Some(
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db"),
    )
}

fn event(lane_id: Uuid, user_id: Uuid, source_id: Option<&str>) -> NewEvent<'_> {
    NewEvent {
        lane_id,
        user_id,
        source: Source::N8n,
        source_id,
        body: "ticket updated",
        payload: None,
        not_before: None,
        expires_at: None,
    }
}

async fn set_status(pool: &PgPool, id: Uuid, status: &str, delivered_days_ago: Option<i32>) {
    sqlx::query(
        "UPDATE inbox_events SET status = $2, \
         delivered_at = now() - make_interval(days => $3) WHERE id = $1",
    )
    .bind(id)
    .bind(status)
    .bind(delivered_days_ago)
    .execute(pool)
    .await
    .unwrap();
}

async fn exists(pool: &PgPool, id: Uuid) -> bool {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM inbox_events WHERE id = $1)")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_lane_is_created_once_per_session() {
    let name = "inbox_lane_once";
    let Some(pool) = pool(name).await else { return };
    let (uid, sid) = seed_session(&pool, name).await;
    let a = session_lane(&pool, uid, &sid).await.unwrap();
    let b = session_lane(&pool, uid, &sid).await.unwrap();
    assert_eq!(a, b);
    let key: String = sqlx::query_scalar("SELECT key FROM inbox_lanes WHERE id = $1")
        .bind(a)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(key, format!("session:{sid}"));
}

#[tokio::test]
async fn a_repeated_source_id_answers_the_existing_event() {
    let name = "inbox_dedup";
    let Some(pool) = pool(name).await else { return };
    let (uid, sid) = seed_session(&pool, name).await;
    let lane = session_lane(&pool, uid, &sid).await.unwrap();

    let first = insert_event(&pool, &event(lane, uid, Some("delivery-1"))).await.unwrap();
    assert!(!first.deduplicated);
    let again = insert_event(&pool, &event(lane, uid, Some("delivery-1"))).await.unwrap();
    assert_eq!((again.id, again.deduplicated), (first.id, true));
    let other = insert_event(&pool, &event(lane, uid, Some("delivery-2"))).await.unwrap();
    assert!(!other.deduplicated);
    assert_ne!(other.id, first.id);

    let timer = NewEvent { source: Source::Timer, ..event(lane, uid, Some("delivery-1")) };
    assert!(!insert_event(&pool, &timer).await.unwrap().deduplicated, "dedup is per source");

    let rows = list(&pool, Some(uid), Some(&format!("session:{sid}")), None).await.unwrap();
    assert_eq!(rows.len(), 3);
    let by_id = list(&pool, Some(uid), Some(&lane.to_string()), None).await.unwrap();
    assert_eq!(by_id.len(), 3);
    let shown = get(&pool, Some(uid), first.id).await.unwrap().expect("own event");
    assert_eq!((shown.status.as_str(), shown.lane_id), ("pending", lane));
}

#[tokio::test]
async fn a_missing_source_id_never_dedups() {
    let name = "inbox_null_source_id";
    let Some(pool) = pool(name).await else { return };
    let (uid, sid) = seed_session(&pool, name).await;
    let lane = session_lane(&pool, uid, &sid).await.unwrap();
    let human = NewEvent { source: Source::Human, ..event(lane, uid, None) };
    let a = insert_event(&pool, &human).await.unwrap();
    let b = insert_event(&pool, &human).await.unwrap();
    assert!(!a.deduplicated && !b.deduplicated);
    assert_ne!(a.id, b.id);
}

#[tokio::test]
async fn only_a_pending_event_can_be_cancelled() {
    let name = "inbox_cancel";
    let Some(pool) = pool(name).await else { return };
    let (uid, sid) = seed_session(&pool, name).await;
    let (stranger, _) = seed_session(&pool, name).await;
    let lane = session_lane(&pool, uid, &sid).await.unwrap();
    let pending = insert_event(&pool, &event(lane, uid, Some("p"))).await.unwrap().id;
    let delivered = insert_event(&pool, &event(lane, uid, Some("d"))).await.unwrap().id;
    set_status(&pool, delivered, "delivered", Some(0)).await;

    assert_eq!(cancel(&pool, Some(stranger), pending).await.unwrap(), Cancel::NotFound);
    assert_eq!(cancel(&pool, Some(uid), pending).await.unwrap(), Cancel::Cancelled);
    assert_eq!(
        cancel(&pool, Some(uid), pending).await.unwrap(),
        Cancel::NotPending("cancelled".to_owned())
    );
    assert_eq!(
        cancel(&pool, Some(uid), delivered).await.unwrap(),
        Cancel::NotPending("delivered".to_owned())
    );
    assert_eq!(cancel(&pool, Some(uid), Uuid::new_v4()).await.unwrap(), Cancel::NotFound);
    assert!(get(&pool, Some(stranger), pending).await.unwrap().is_none());
    assert_eq!(
        cancel(&pool, None, delivered).await.unwrap(),
        Cancel::NotPending("delivered".to_owned()),
        "an admin is refused too"
    );
}

#[tokio::test]
async fn prune_takes_only_delivered_events_past_retention() {
    let name = "inbox_prune";
    let Some(pool) = pool(name).await else { return };
    let (uid, sid) = seed_session(&pool, name).await;
    let lane = session_lane(&pool, uid, &sid).await.unwrap();
    let mut ids = Vec::new();
    for (source_id, status, days) in [
        ("old-delivered", "delivered", Some(31)),
        ("young-delivered", "delivered", Some(29)),
        ("old-dead", "dead", Some(31)),
        ("old-cancelled", "cancelled", Some(31)),
        ("pending", "pending", None),
    ] {
        let id = insert_event(&pool, &event(lane, uid, Some(source_id))).await.unwrap().id;
        set_status(&pool, id, status, days).await;
        ids.push(id);
    }
    while prune_delivered(&pool).await.unwrap() > 0 {}
    let mut kept = Vec::new();
    for id in ids {
        kept.push(exists(&pool, id).await);
    }
    assert_eq!(kept, [false, true, true, true, true]);
}

#[tokio::test]
async fn only_a_dead_event_can_be_retried_by_its_owner() {
    let name = "inbox_retry";
    let Some(pool) = pool(name).await else { return };
    let (uid, sid) = seed_session(&pool, name).await;
    let (stranger, _) = seed_session(&pool, name).await;
    let lane = session_lane(&pool, uid, &sid).await.unwrap();
    let dead = insert_event(&pool, &event(lane, uid, Some("x"))).await.unwrap().id;
    let delivered = insert_event(&pool, &event(lane, uid, Some("d"))).await.unwrap().id;
    set_status(&pool, delivered, "delivered", Some(0)).await;
    sqlx::query(
        "UPDATE inbox_events SET status = 'dead', attempts = 11, last_error = 'offline', \
         batch_id = gen_random_uuid(), next_attempt_at = now() + interval '1 day' WHERE id = $1",
    )
    .bind(dead)
    .execute(&pool)
    .await
    .unwrap();

    assert_eq!(retry(&pool, Some(stranger), dead).await.unwrap(), Retry::NotFound);
    assert_eq!(retry(&pool, Some(uid), dead).await.unwrap(), Retry::Retried { lane_id: lane });
    let row: (String, i32, Option<String>, Option<Uuid>, bool) = sqlx::query_as(
        "SELECT status, attempts, last_error, batch_id, next_attempt_at <= now() \
         FROM inbox_events WHERE id = $1",
    )
    .bind(dead)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row, ("pending".to_owned(), 0, None, None, true));
    assert_eq!(retry(&pool, Some(uid), dead).await.unwrap(), Retry::NotDead("pending".to_owned()));
    assert_eq!(
        retry(&pool, Some(uid), delivered).await.unwrap(),
        Retry::NotDead("delivered".to_owned())
    );
    assert_eq!(retry(&pool, Some(uid), Uuid::new_v4()).await.unwrap(), Retry::NotFound);
}
