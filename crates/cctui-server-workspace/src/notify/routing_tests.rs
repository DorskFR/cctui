use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::{Edge, NewNotification, Routing, emit, transition};
use crate::notify::kind;

const ON: Routing = Routing { ntfy: true };

async fn db(name: &str) -> Option<PgPool> {
    let url = crate::routes::gateway::test_db_url(name)?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .expect("connect test db");
    Some(pool)
}

async fn user(pool: &PgPool, settings: Option<Value>) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(format!("routing-{id}"))
        .bind(format!("routing-key-{id}"))
        .execute(pool)
        .await
        .unwrap();
    if let Some(data) = settings {
        set_settings(pool, id, data).await;
    }
    id
}

async fn set_settings(pool: &PgPool, user_id: Uuid, data: Value) {
    sqlx::query(
        "INSERT INTO user_settings (user_id, version, data) VALUES ($1, 1, $2) \
         ON CONFLICT (user_id) DO UPDATE SET data = EXCLUDED.data",
    )
    .bind(user_id)
    .bind(data)
    .execute(pool)
    .await
    .unwrap();
}

async fn drop_user(pool: &PgPool, id: Uuid) {
    sqlx::query("DELETE FROM user_settings WHERE user_id = $1")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE id = $1").bind(id).execute(pool).await.unwrap();
}

/// `(channel, state, stage, seconds until due)` per delivery.
async fn deliveries(pool: &PgPool, notification: Uuid) -> Vec<(String, String, i32, f64)> {
    sqlx::query_as(
        "SELECT channel, state, stage, \
                EXTRACT(EPOCH FROM next_attempt_at - now())::float8 \
         FROM notification_deliveries WHERE notification_id = $1 ORDER BY channel",
    )
    .bind(notification)
    .fetch_all(pool)
    .await
    .unwrap()
}

fn alert(u: Uuid, title: &str) -> NewNotification {
    NewNotification::new(u, kind::SYSTEM_ALERT, title.into())
}

#[tokio::test]
async fn a_user_with_ntfy_off_gets_no_ntfy_row() {
    let Some(pool) = db("notify_routing_ntfy_off").await else { return };
    let off = user(&pool, Some(json!({ "notifications": {
        "channels": { "ntfy": { "enabled": false } } } })))
    .await;
    let plain = user(&pool, None).await;

    let muted = emit(&pool, ON, alert(off, "a")).await.unwrap();
    assert!(deliveries(&pool, muted.id).await.is_empty());
    let routed = emit(&pool, ON, alert(plain, "a")).await.unwrap();
    let rows = deliveries(&pool, routed.id).await;
    assert_eq!(rows.len(), 1, "no settings keeps today's routing");
    assert_eq!((rows[0].0.as_str(), rows[0].2), ("ntfy", 0));
    drop_user(&pool, off).await;
    drop_user(&pool, plain).await;
}

#[tokio::test]
async fn the_master_switch_off_plans_nothing_and_writes_the_notice_read() {
    let Some(pool) = db("notify_routing_master_off").await else { return };
    let u = user(&pool, Some(json!({ "notifications": { "enabled": false } }))).await;
    let record = emit(&pool, ON, alert(u, "a")).await.unwrap();
    assert!(deliveries(&pool, record.id).await.is_empty());
    assert_eq!(record.state, "read");
    assert!(record.read_at.is_some());
    drop_user(&pool, u).await;
}

#[tokio::test]
async fn in_app_off_for_a_kind_writes_it_read_but_still_pushes() {
    let Some(pool) = db("notify_routing_in_app_off").await else { return };
    let u = user(&pool, Some(json!({ "notifications": { "routes": {
        "system_alert": { "in_app": { "mode": "off" } } } } })))
    .await;
    let record = emit(&pool, ON, alert(u, "a")).await.unwrap();
    assert_eq!(record.state, "read");
    assert_eq!(deliveries(&pool, record.id).await.len(), 1);
    drop_user(&pool, u).await;
}

#[tokio::test]
async fn after_ten_gives_a_stage_one_row_ten_minutes_out_and_read_cancels_it() {
    let Some(pool) = db("notify_routing_after_ten").await else { return };
    let u = user(&pool, Some(json!({ "notifications": { "routes": {
        "system_alert": { "ntfy": { "mode": "after", "minutes": 10 } } } } })))
    .await;
    let record = emit(&pool, ON, alert(u, "a")).await.unwrap();
    let rows = deliveries(&pool, record.id).await;
    assert_eq!(rows.len(), 1);
    let (channel, state, stage, due_in) = &rows[0];
    assert_eq!((channel.as_str(), state.as_str(), *stage), ("ntfy", "scheduled", 1));
    assert!((570.0..=600.0).contains(due_in), "due in {due_in}s");

    transition(&pool, record.id, Edge::Read).await.unwrap().unwrap();
    let rows = deliveries(&pool, record.id).await;
    assert_eq!(rows[0].1, "cancelled", "reading it in the app cancels the escalation");
    drop_user(&pool, u).await;
}

#[tokio::test]
async fn acting_or_resolving_cancels_a_pending_escalation() {
    let Some(pool) = db("notify_routing_resolve_cancels").await else { return };
    let u = user(&pool, None).await;
    let n = NewNotification::new(u, kind::APPROVAL, "Approve?".into());
    let record = emit(&pool, ON, n).await.unwrap();
    assert_eq!(deliveries(&pool, record.id).await[0].1, "scheduled");
    transition(&pool, record.id, Edge::Resolve { reason: "subject_gone" }).await.unwrap();
    assert_eq!(deliveries(&pool, record.id).await[0].1, "cancelled");
    drop_user(&pool, u).await;
}

async fn drop_seeded(pool: &PgPool, u: Uuid, sid: &str) {
    sqlx::query("DELETE FROM sessions WHERE id = $1").bind(sid).execute(pool).await.unwrap();
    drop_user(pool, u).await;
}

async fn view(pool: &PgPool, sid: &str) {
    sqlx::query("INSERT INTO session_viewers (session_id, pod) VALUES ($1, 'routing-test')")
        .bind(sid)
        .execute(pool)
        .await
        .unwrap();
}

async fn unview(pool: &PgPool, sid: &str) {
    sqlx::query("DELETE FROM session_viewers WHERE session_id = $1")
        .bind(sid)
        .execute(pool)
        .await
        .unwrap();
}

fn on_session(u: Uuid, sid: &str, notice_kind: &'static str) -> NewNotification {
    NewNotification {
        session_id: Some(sid.to_owned()),
        ..NewNotification::new(u, notice_kind, "look".into())
    }
}

#[tokio::test]
async fn a_watched_session_keeps_the_approval_delay() {
    let Some(pool) = db("notify_routing_presence_kept").await else { return };
    let (u, sid) = crate::inbox::store_tests::seed_session(&pool, "routing-watched").await;
    view(&pool, &sid).await;
    let record = emit(&pool, ON, on_session(u, &sid, kind::APPROVAL)).await.unwrap();
    let (_, _, stage, due_in) = deliveries(&pool, record.id).await.remove(0);
    assert_eq!(stage, 1);
    assert!(due_in > 270.0, "the human is looking: wait the 5 minutes ({due_in}s)");
    unview(&pool, &sid).await;
    drop_seeded(&pool, u, &sid).await;
}

#[tokio::test]
async fn no_live_socket_drops_stage_one_to_zero_for_approval_and_session_stuck() {
    let Some(pool) = db("notify_routing_presence_absent").await else { return };
    let (u, sid) = crate::inbox::store_tests::seed_session(&pool, "routing-absent").await;
    set_settings(&pool, u, json!({ "notifications": { "routes": {
        "session_stuck": { "ntfy": { "mode": "after", "minutes": 10 } } } } }))
    .await;
    for notice_kind in [kind::APPROVAL, kind::SESSION_STUCK] {
        let record = emit(&pool, ON, on_session(u, &sid, notice_kind)).await.unwrap();
        let (_, _, stage, due_in) = deliveries(&pool, record.id).await.remove(0);
        assert_eq!(stage, 1, "{notice_kind}");
        assert!(due_in <= 1.0, "{notice_kind}: nobody is looking, push now ({due_in}s)");
    }
    let record = emit(&pool, ON, on_session(u, &sid, kind::NEEDS_INPUT)).await.unwrap();
    let (_, _, _, due_in) = deliveries(&pool, record.id).await.remove(0);
    assert!(due_in > 570.0, "needs_input keeps its wait ({due_in}s)");
    drop_seeded(&pool, u, &sid).await;
}

async fn throttled_rows(pool: &PgPool, u: Uuid) -> Vec<(Uuid, i32)> {
    sqlx::query_as("SELECT id, repeat_count FROM notifications WHERE user_id = $1 AND kind = $2")
        .bind(u)
        .bind(kind::THROTTLED)
        .fetch_all(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn the_twenty_first_push_in_an_hour_stays_in_app_with_one_throttled_entry() {
    let Some(pool) = db("notify_routing_push_cap").await else { return };
    let u = user(&pool, None).await;
    for i in 0..20 {
        let record = emit(&pool, ON, alert(u, &format!("alert {i}"))).await.unwrap();
        assert_eq!(deliveries(&pool, record.id).await.len(), 1, "push {i}");
    }
    assert!(throttled_rows(&pool, u).await.is_empty());

    let over = emit(&pool, ON, alert(u, "alert 20")).await.unwrap();
    assert!(deliveries(&pool, over.id).await.is_empty(), "the 21st stays in the app");
    let throttled = throttled_rows(&pool, u).await;
    assert_eq!(throttled.len(), 1);
    assert!(deliveries(&pool, throttled[0].0).await.is_empty(), "the warning never pushes");

    let again = emit(&pool, ON, alert(u, "alert 21")).await.unwrap();
    assert!(deliveries(&pool, again.id).await.is_empty());
    let throttled = throttled_rows(&pool, u).await;
    assert_eq!(throttled.len(), 1, "still one throttled entry");
    assert_eq!(throttled[0].1, 2);
    drop_user(&pool, u).await;
}

#[tokio::test]
async fn a_collapse_onto_a_pushing_notice_is_never_throttled() {
    let Some(pool) = db("notify_routing_cap_collapse").await else { return };
    let u = user(&pool, Some(json!({ "notifications": { "max_push_per_hour": 1 } }))).await;
    let crashloop = |title: &str| NewNotification {
        collapse_key: Some(format!("crashloop:{u}")),
        ..alert(u, title)
    };
    let first = emit(&pool, ON, crashloop("x1")).await.unwrap();
    emit(&pool, ON, alert(u, "other")).await.unwrap();
    let second = emit(&pool, ON, crashloop("x2")).await.unwrap();
    assert_eq!(second.id, first.id);
    assert_eq!(deliveries(&pool, first.id).await.len(), 1);
    drop_user(&pool, u).await;
}

async fn utc_minute(pool: &PgPool) -> u32 {
    let mut conn = pool.acquire().await.unwrap();
    crate::notify::quiet::local_minute(&mut conn, 0, "UTC").await.unwrap()
}

#[tokio::test]
async fn quiet_hours_hold_a_push_until_the_window_ends_but_not_a_critical_one() {
    let Some(pool) = db("notify_routing_quiet").await else { return };
    let now = utc_minute(&pool).await;
    let fmt = |m: u32| format!("{:02}:{:02}", (m / 60) % 24, m % 60);
    let window = json!({
        "start": fmt((now + 24 * 60 - 30) % (24 * 60)),
        "end": fmt((now + 90) % (24 * 60)),
        "tz": "UTC",
    });
    let u = user(&pool, Some(json!({ "notifications": { "quiet_hours": window } }))).await;

    let held = emit(&pool, ON, alert(u, "quiet please")).await.unwrap();
    let (_, state, stage, due_in) = deliveries(&pool, held.id).await.remove(0);
    assert_eq!((state.as_str(), stage), ("scheduled", 1));
    assert!((85.0 * 60.0..=90.0 * 60.0).contains(&due_in), "held to the window end ({due_in}s)");

    let critical = NewNotification { severity: "critical", ..alert(u, "fire") };
    let loud = emit(&pool, ON, critical).await.unwrap();
    let (_, _, stage, due_in) = deliveries(&pool, loud.id).await.remove(0);
    assert_eq!(stage, 0);
    assert!(due_in <= 1.0, "critical goes out now ({due_in}s)");
    drop_user(&pool, u).await;
}
