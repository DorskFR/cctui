use cctui_proto::ws::ServerEvent;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use super::{record_event, test_bus, unread_event};
use crate::notify::store::{Edge, NewNotification, NotificationRecord, Routing, emit, transition};

fn sample() -> NotificationRecord {
    serde_json::from_value(json!({
        "id": Uuid::from_u128(1),
        "user_id": Uuid::from_u128(2),
        "kind": "approval",
        "severity": "high",
        "title": "Release?",
        "actions": [{ "id": "approve", "label": "Approve" }],
        "state": "read",
        "repeat_count": 2,
        "created_at": "2026-11-16T10:00:00Z",
        "updated_at": "2026-11-16T10:05:00Z",
    }))
    .unwrap()
}

#[test]
fn the_record_event_carries_the_whole_record() {
    let n = sample();
    let ServerEvent::Notification { notification } = record_event(&n) else {
        panic!("not a notification event")
    };
    assert_eq!(*notification, n);
    let wire = serde_json::to_value(record_event(&n)).unwrap();
    assert_eq!(wire["type"], "notification");
    assert_eq!(wire["notification"]["state"], "read");
    let wire = serde_json::to_value(unread_event(n.user_id, 3)).unwrap();
    assert_eq!(wire, json!({ "type": "notifications_unread", "user_id": n.user_id, "count": 3 }));
}

async fn db(name: &str) -> Option<PgPool> {
    let url = cctui_server_kernel::test_support::db_url(name)?;
    Some(PgPool::connect(&url).await.expect("connect test db"))
}

async fn user(pool: &PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(format!("in-app-{id}"))
        .bind(format!("in-app-key-{id}"))
        .execute(pool)
        .await
        .unwrap();
    id
}

#[tokio::test]
async fn every_transition_publishes_the_record_and_the_unread_count() {
    let Some(pool) = db("notify_in_app_publish").await else { return };
    let mut rx = test_bus().subscribe_server();
    let owner = user(&pool).await;
    let mut n = NewNotification::new(owner, "approval", "Release?".into());
    n.actions = json!([{ "id": "approve", "label": "Approve" }]);
    let id = emit(&pool, Routing::default(), n).await.unwrap().id;
    let approve = Edge::Act { action: "approve", note: None, channel: "web", by: Some(owner) };
    for edge in [Edge::Deliver, Edge::Read, approve, approve, Edge::Resolve { reason: "done" }] {
        transition(&pool, id, edge).await.unwrap().unwrap();
    }

    let (mut states, mut counts) = (Vec::new(), Vec::new());
    while let Ok(frame) = rx.try_recv() {
        match &*frame.event {
            ServerEvent::Notification { notification } if notification.id == id => {
                states.push(notification.state.clone());
            }
            ServerEvent::NotificationsUnread { user_id, count } if *user_id == owner => {
                counts.push(*count);
            }
            _ => {}
        }
    }
    assert_eq!(states, ["pending", "delivered", "read", "acted", "resolved"]);
    assert_eq!(counts, [1, 1, 0, 0, 0], "a lost race publishes nothing");

    sqlx::query("DELETE FROM users WHERE id = $1").bind(owner).execute(&pool).await.unwrap();
}
