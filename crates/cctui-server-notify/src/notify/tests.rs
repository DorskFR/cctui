use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::channel::{Capabilities, Channel, Channels, Endpoint, ProviderRef, Rendered, SendError};
use super::dispatcher::{MAX_ATTEMPTS, run};
use super::store::{
    BODY_MAX_CHARS, Edge, NewNotification, Routing, Transitioned, clip, emit, needs_answer,
    read_all, transition, unread_count,
};
use super::{kind, retention};

const APPROVE: Edge<'static> =
    Edge::Act { action: "approve", note: None, channel: "web", by: None };
const REJECT: Edge<'static> =
    Edge::Act { action: "reject", note: Some("no"), channel: "ntfy", by: None };

async fn db(name: &str) -> Option<PgPool> {
    let url = cctui_server_kernel::test_support::db_url(name)?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .expect("connect test db");
    Some(pool)
}

async fn user(pool: &PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(format!("notify-{id}"))
        .bind(format!("notify-key-{id}"))
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn drop_user(pool: &PgPool, id: Uuid) {
    sqlx::query("DELETE FROM users WHERE id = $1").bind(id).execute(pool).await.unwrap();
}

async fn open(pool: &PgPool, user: Uuid) -> Uuid {
    let n = NewNotification::new(user, "test", "t".into());
    emit(pool, Routing::default(), n).await.unwrap().id
}

async fn apply(pool: &PgPool, id: Uuid, edge: Edge<'_>) -> Transitioned {
    transition(pool, id, edge).await.unwrap().unwrap()
}

async fn delivery(
    pool: &PgPool,
    notification: Uuid,
    channel: &str,
    op: Option<&str>,
    state: &str,
    provider_ref: Option<Value>,
) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO notification_deliveries (notification_id, channel, op, state, provider_ref) \
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(notification)
    .bind(channel)
    .bind(op)
    .bind(state)
    .bind(provider_ref)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[derive(Debug, sqlx::FromRow)]
struct DeliveryRow {
    op: Option<String>,
    state: String,
    attempts: i32,
    provider_ref: Option<Value>,
    due_later: bool,
    last_error: Option<String>,
}

async fn delivery_row(pool: &PgPool, id: Uuid) -> DeliveryRow {
    sqlx::query_as(
        "SELECT op, state, attempts, provider_ref, next_attempt_at > now() AS due_later, \
         last_error FROM notification_deliveries WHERE id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn token(pool: &PgPool, notification: Uuid) -> Vec<u8> {
    let hash = Uuid::new_v4().as_bytes().to_vec();
    sqlx::query(
        "INSERT INTO notification_action_tokens \
         (token_hash, notification_id, action, channel, expires_at) \
         VALUES ($1, $2, 'approve', 'ntfy', now() + interval '1 hour')",
    )
    .bind(&hash)
    .bind(notification)
    .execute(pool)
    .await
    .unwrap();
    hash
}

#[test]
fn bodies_are_clipped_to_280_chars() {
    assert_eq!(clip("short"), "short");
    let exact = "é".repeat(BODY_MAX_CHARS);
    assert_eq!(clip(&exact), exact);
    let long = clip(&"é".repeat(BODY_MAX_CHARS + 1));
    assert_eq!(long.chars().count(), BODY_MAX_CHARS);
    assert!(long.ends_with('…'));
}

#[tokio::test]
async fn every_legal_edge_applies() {
    let Some(pool) = db("notify_every_legal_edge").await else { return };
    let u = user(&pool).await;
    let paths: &[&[Edge<'_>]] = &[
        &[Edge::Deliver, Edge::Read, APPROVE, Edge::Resolve { reason: "acknowledged" }],
        &[Edge::Read],
        &[APPROVE],
        &[Edge::Deliver, APPROVE],
        &[Edge::Dismiss],
        &[Edge::Deliver, Edge::Dismiss],
        &[Edge::Read, Edge::Dismiss],
        &[Edge::Resolve { reason: "superseded" }],
        &[Edge::Expire],
        &[Edge::Deliver, Edge::Expire],
        &[Edge::Read, Edge::Expire],
    ];
    for path in paths {
        let id = open(&pool, u).await;
        for edge in *path {
            let t = apply(&pool, id, *edge).await;
            assert!(t.applied, "{path:?}: {edge:?}");
            assert_eq!(t.notification.state, edge.target().as_str(), "{path:?}");
        }
    }

    let acted = apply(&pool, open(&pool, u).await, REJECT).await.notification;
    assert_eq!(acted.decision.as_deref(), Some("reject"));
    assert_eq!(acted.decision_note.as_deref(), Some("no"));
    assert_eq!(acted.acted_channel.as_deref(), Some("ntfy"));
    assert!(acted.acted_at.is_some() && acted.read_at.is_some());

    let dismissed = apply(&pool, open(&pool, u).await, Edge::Dismiss).await.notification;
    assert_eq!(dismissed.decision.as_deref(), Some("dismissed"));
    assert_eq!(dismissed.resolved_reason.as_deref(), Some("dismissed"));
    assert!(dismissed.resolved_at.is_some());

    let expired = apply(&pool, open(&pool, u).await, Edge::Expire).await.notification;
    assert_eq!(expired.decision.as_deref(), Some("expired"));
    drop_user(&pool, u).await;
}

async fn escalation(pool: &PgPool, notification: Uuid) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO notification_deliveries (notification_id, channel, op, state, stage) \
         VALUES ($1, 'ntfy', 'send', 'scheduled', 1) RETURNING id",
    )
    .bind(notification)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn read_all_reads_actionable_notices_but_leaves_them_answerable() {
    let Some(pool) = db("notify_read_all").await else { return };
    let u = user(&pool).await;
    let info = open(&pool, u).await;
    let mut ask = NewNotification::new(u, kind::APPROVAL, "Ship?".into());
    ask.actions = json!([{ "id": "approve", "label": "Approve" }]);
    let ask = emit(&pool, Routing::default(), ask).await.unwrap().id;
    let info_push = escalation(&pool, info).await;
    let ask_push = escalation(&pool, ask).await;

    let read = read_all(&pool, u).await.unwrap();
    assert_eq!(read.len(), 2);
    assert!(read.iter().all(|n| n.state == "read" && n.read_at.is_some()));
    assert_eq!(unread_count(&pool, u).await.unwrap(), 0);
    let asked = read.iter().find(|n| n.id == ask).unwrap();
    assert!(needs_answer(asked));
    assert!(asked.decision.is_none());
    assert!(!needs_answer(read.iter().find(|n| n.id == info).unwrap()));
    assert_eq!(delivery_row(&pool, info_push).await.state, "cancelled");
    assert_eq!(delivery_row(&pool, ask_push).await.state, "scheduled");

    let acted = apply(&pool, ask, APPROVE).await;
    assert!(acted.applied);
    assert_eq!(acted.notification.decision.as_deref(), Some("approve"));
    assert!(read_all(&pool, u).await.unwrap().is_empty());
    drop_user(&pool, u).await;
}

#[tokio::test]
async fn an_illegal_edge_returns_the_current_row_unapplied() {
    let Some(pool) = db("notify_illegal_edge").await else { return };
    let u = user(&pool).await;
    let id = open(&pool, u).await;
    assert!(apply(&pool, id, Edge::Deliver).await.applied);
    let again = apply(&pool, id, Edge::Deliver).await;
    assert!(!again.applied);
    assert_eq!(again.notification.state, "delivered");

    assert!(apply(&pool, id, APPROVE).await.applied);
    for edge in [Edge::Read, Edge::Deliver, Edge::Dismiss, Edge::Expire, REJECT] {
        let t = apply(&pool, id, edge).await;
        assert!(!t.applied, "{edge:?}");
        assert_eq!(t.notification.state, "acted", "{edge:?}");
        assert_eq!(t.notification.decision.as_deref(), Some("approve"), "{edge:?}");
    }
    assert!(apply(&pool, id, Edge::Resolve { reason: "acknowledged" }).await.applied);
    assert!(!apply(&pool, id, Edge::Resolve { reason: "acknowledged" }).await.applied);

    assert!(transition(&pool, Uuid::new_v4(), Edge::Read).await.unwrap().is_none());
    drop_user(&pool, u).await;
}

#[tokio::test]
async fn two_concurrent_acts_resolve_exactly_once() {
    let Some(pool) = db("notify_concurrent_act").await else { return };
    let u = user(&pool).await;
    for _ in 0..10 {
        let id = open(&pool, u).await;
        let (a, b) = tokio::join!(transition(&pool, id, APPROVE), transition(&pool, id, REJECT));
        let (a, b) = (a.unwrap().unwrap(), b.unwrap().unwrap());
        assert_eq!(u8::from(a.applied) + u8::from(b.applied), 1, "{a:?} {b:?}");
        assert_eq!(a.notification.decision, b.notification.decision);
        assert_eq!(a.notification.state, "acted");
        assert_eq!(b.notification.state, "acted");
    }
    drop_user(&pool, u).await;
}

#[tokio::test]
async fn a_collapse_bumps_repeat_count_instead_of_inserting() {
    let Some(pool) = db("notify_collapse").await else { return };
    let u = user(&pool).await;
    let notice = |title: &str| NewNotification {
        collapse_key: Some(format!("crashloop:{u}")),
        body: Some(title.repeat(2)),
        ..NewNotification::new(u, "test", title.into())
    };
    let first = emit(&pool, Routing::default(), notice("one")).await.unwrap();
    let second = emit(&pool, Routing::default(), notice("two")).await.unwrap();
    assert_eq!(second.id, first.id);
    assert_eq!(second.repeat_count, 2);
    assert_eq!((second.title.as_str(), second.body.as_deref()), ("two", Some("twotwo")));
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM notifications WHERE user_id = $1")
        .bind(u)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 1);

    apply(&pool, first.id, Edge::Dismiss).await;
    let third = emit(&pool, Routing::default(), notice("three")).await.unwrap();
    assert_ne!(third.id, first.id, "a closed notice does not absorb new ones");
    assert_eq!(third.repeat_count, 1);
    drop_user(&pool, u).await;
}

#[tokio::test]
async fn closing_retracts_sent_cancels_unsent_and_burns_tokens() {
    let Some(pool) = db("notify_close_side_effects").await else { return };
    let u = user(&pool).await;
    let id = open(&pool, u).await;
    let sent = delivery(&pool, id, "a", None, "sent", Some(json!({ "r": 1 }))).await;
    let unsent = delivery(&pool, id, "b", Some("send"), "scheduled", None).await;
    let hash = token(&pool, id).await;

    assert!(apply(&pool, id, APPROVE).await.applied);

    let sent = delivery_row(&pool, sent).await;
    assert_eq!((sent.op.as_deref(), sent.state.as_str()), (Some("retract"), "sent"));
    let unsent = delivery_row(&pool, unsent).await;
    assert_eq!((unsent.op.as_deref(), unsent.state.as_str()), (None, "cancelled"));
    let burnt: bool = sqlx::query_scalar(
        "SELECT used_at IS NOT NULL FROM notification_action_tokens WHERE token_hash = $1",
    )
    .bind(&hash)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(burnt);
    drop_user(&pool, u).await;
}

#[tokio::test]
async fn system_alerts_route_to_the_env_ntfy_endpoint_only_when_configured() {
    let Some(pool) = db("notify_legacy_routing").await else { return };
    let u = user(&pool).await;
    let ping = |title: &str| NewNotification {
        collapse_key: Some("alert-test".into()),
        body: Some("x".repeat(400)),
        ..NewNotification::new(u, kind::SYSTEM_ALERT, title.into())
    };
    let on = Routing { ntfy: true, env_owner: Some(u) };
    let record = emit(&pool, on, ping("a")).await.unwrap();
    assert_eq!(record.body.as_deref().map(|b| b.chars().count()), Some(BODY_MAX_CHARS));
    let (channel, op, state, config): (String, Option<String>, String, Value) = sqlx::query_as(
        "SELECT d.channel, d.op, d.state, e.config FROM notification_deliveries d \
         JOIN notification_endpoints e ON e.id = d.endpoint_id WHERE d.notification_id = $1",
    )
    .bind(record.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        (channel.as_str(), op.as_deref(), state.as_str()),
        ("ntfy", Some("send"), "scheduled")
    );
    assert_eq!(config, json!({ "source": "env" }));

    sqlx::query(
        "UPDATE notification_deliveries SET state = 'sent', op = NULL WHERE notification_id = $1",
    )
    .bind(record.id)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(emit(&pool, on, ping("b")).await.unwrap().id, record.id);
    let ops: Vec<Option<String>> =
        sqlx::query_scalar("SELECT op FROM notification_deliveries WHERE notification_id = $1")
            .bind(record.id)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(ops, [Some("update".to_owned())], "a collapse replaces the sent message");
    let endpoints: i64 =
        sqlx::query_scalar("SELECT count(*) FROM notification_endpoints WHERE user_id = $1")
            .bind(u)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(endpoints, 1);

    let off = NewNotification::new(u, kind::SYSTEM_ALERT, "quiet".into());
    let quiet = emit(&pool, Routing::default(), off).await.unwrap();
    let none: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notification_deliveries WHERE notification_id = $1",
    )
    .bind(quiet.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(none, 0);
    drop_user(&pool, u).await;
}

#[derive(Default)]
struct Fake {
    calls: Mutex<Vec<&'static str>>,
    fail: Option<SendError>,
}

impl Fake {
    fn failing(e: SendError) -> Arc<Self> {
        Arc::new(Self { fail: Some(e), ..Self::default() })
    }

    fn calls(&self) -> Vec<&'static str> {
        self.calls.lock().unwrap().clone()
    }

    fn answer<T>(&self, call: &'static str, ok: T) -> Result<T, SendError> {
        self.calls.lock().unwrap().push(call);
        self.fail.clone().map_or(Ok(ok), Err)
    }

    fn channels(self: &Arc<Self>) -> Channels {
        let mut channels = Channels::default();
        channels.add(self.clone());
        channels
    }
}

#[async_trait::async_trait]
impl Channel for Fake {
    fn id(&self) -> &'static str {
        "fake"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities { update: true, retract: true, ..Capabilities::default() }
    }

    async fn send(&self, n: &Rendered, _: &Endpoint) -> Result<ProviderRef, SendError> {
        self.answer("send", ProviderRef(json!({ "id": n.delivery_id })))
    }

    async fn update(&self, _: &Rendered, _: &Endpoint, _: &ProviderRef) -> Result<(), SendError> {
        self.answer("update", ())
    }

    async fn retract(&self, _: &Endpoint, _: &ProviderRef) -> Result<(), SendError> {
        self.answer("retract", ())
    }
}

async fn notification_state(pool: &PgPool, id: Uuid) -> String {
    sqlx::query_scalar("SELECT state FROM notifications WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_successful_send_stores_the_provider_ref_and_marks_delivered() {
    let Some(pool) = db("notify_dispatch_send").await else { return };
    let u = user(&pool).await;
    let id = open(&pool, u).await;
    let d = delivery(&pool, id, "fake", Some("send"), "scheduled", None).await;
    let fake = Arc::new(Fake::default());
    run(&pool, &fake.channels(), Some(id)).await;

    let row = delivery_row(&pool, d).await;
    assert_eq!((row.op.as_deref(), row.state.as_str()), (None, "sent"));
    assert_eq!(row.provider_ref, Some(json!({ "id": d })));
    assert_eq!(notification_state(&pool, id).await, "delivered");
    assert_eq!(fake.calls(), ["send"]);
    drop_user(&pool, u).await;
}

#[tokio::test]
async fn a_retryable_failure_schedules_a_later_attempt() {
    let Some(pool) = db("notify_dispatch_retry").await else { return };
    let u = user(&pool).await;
    let id = open(&pool, u).await;
    let d = delivery(&pool, id, "fake", Some("send"), "scheduled", None).await;
    let fake = Fake::failing(SendError::Retry("down".into()));
    run(&pool, &fake.channels(), Some(id)).await;

    let row = delivery_row(&pool, d).await;
    assert_eq!((row.op.as_deref(), row.state.as_str()), (Some("send"), "scheduled"));
    assert_eq!((row.attempts, row.due_later), (1, true));
    assert_eq!(row.last_error.as_deref(), Some("down"));
    assert_eq!(notification_state(&pool, id).await, "pending");

    sqlx::query(
        "UPDATE notification_deliveries SET attempts = $2, next_attempt_at = now() WHERE id = $1",
    )
    .bind(d)
    .bind(MAX_ATTEMPTS - 1)
    .execute(&pool)
    .await
    .unwrap();
    run(&pool, &fake.channels(), Some(id)).await;
    let row = delivery_row(&pool, d).await;
    assert_eq!((row.op.as_deref(), row.state.as_str()), (None, "dead"), "budget spent");
    drop_user(&pool, u).await;
}

#[tokio::test]
async fn a_permanent_failure_dead_letters() {
    let Some(pool) = db("notify_dispatch_permanent").await else { return };
    let u = user(&pool).await;
    let id = open(&pool, u).await;
    let d = delivery(&pool, id, "fake", Some("send"), "scheduled", None).await;
    run(&pool, &Fake::failing(SendError::Permanent("bad".into())).channels(), Some(id)).await;
    let row = delivery_row(&pool, d).await;
    assert_eq!((row.op.as_deref(), row.state.as_str()), (None, "dead"));
    assert_eq!(row.last_error.as_deref(), Some("bad"));

    let other = open(&pool, u).await;
    let d = delivery(&pool, other, "fake", Some("send"), "scheduled", None).await;
    run(&pool, &Channels::default(), Some(other)).await;
    let row = delivery_row(&pool, d).await;
    assert_eq!(row.state, "dead", "an unconfigured channel never sends");
    drop_user(&pool, u).await;
}

#[tokio::test]
async fn gone_disables_the_endpoint() {
    let Some(pool) = db("notify_dispatch_gone").await else { return };
    let u = user(&pool).await;
    let id = open(&pool, u).await;
    let endpoint: Uuid = sqlx::query_scalar(
        "INSERT INTO notification_endpoints (user_id, channel, config) \
         VALUES ($1, 'fake', '{}') RETURNING id",
    )
    .bind(u)
    .fetch_one(&pool)
    .await
    .unwrap();
    let d = delivery(&pool, id, "fake", Some("send"), "scheduled", None).await;
    sqlx::query("UPDATE notification_deliveries SET endpoint_id = $2 WHERE id = $1")
        .bind(d)
        .bind(endpoint)
        .execute(&pool)
        .await
        .unwrap();
    run(&pool, &Fake::failing(SendError::Gone).channels(), Some(id)).await;
    let enabled: bool =
        sqlx::query_scalar("SELECT enabled FROM notification_endpoints WHERE id = $1")
            .bind(endpoint)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!enabled);
    assert_eq!(delivery_row(&pool, d).await.state, "dead");
    drop_user(&pool, u).await;
}

#[tokio::test]
async fn retract_cancels_an_unsent_delivery_and_retracts_a_sent_one() {
    let Some(pool) = db("notify_dispatch_retract").await else { return };
    let u = user(&pool).await;
    let id = open(&pool, u).await;
    let unsent = delivery(&pool, id, "fake", Some("retract"), "scheduled", None).await;
    let fake = Arc::new(Fake::default());
    run(&pool, &fake.channels(), Some(id)).await;
    let row = delivery_row(&pool, unsent).await;
    assert_eq!((row.op.as_deref(), row.state.as_str()), (None, "cancelled"));
    assert!(fake.calls().is_empty(), "the adapter is not called for an unsent delivery");

    let other = open(&pool, u).await;
    let sent_ref = Some(json!({ "id": 1 }));
    let sent = delivery(&pool, other, "fake", Some("retract"), "sent", sent_ref).await;
    run(&pool, &fake.channels(), Some(other)).await;
    let row = delivery_row(&pool, sent).await;
    assert_eq!((row.op.as_deref(), row.state.as_str()), (None, "retracted"));
    assert_eq!(fake.calls(), ["retract"]);
    drop_user(&pool, u).await;
}

async fn aged(pool: &PgPool, user: Uuid, state: &str, days: i32) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO notifications (user_id, kind, title, state, created_at, resolved_at) \
         VALUES ($1, 'test', 't', $2, now() - make_interval(days => $3), \
                 CASE WHEN $2 IN ('resolved', 'expired') \
                      THEN now() - make_interval(days => $3) END) \
         RETURNING id",
    )
    .bind(user)
    .bind(state)
    .bind(days)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn retention_keeps_deliveries_the_push_cap_still_counts() {
    let Some(pool) = db("notify_retention_cap").await else { return };
    let u = user(&pool).await;
    let just_resolved: Uuid = sqlx::query_scalar(
        "INSERT INTO notifications (user_id, kind, title, state, resolved_at) \
         VALUES ($1, 'test', 't', 'resolved', now() - interval '5 minutes') RETURNING id",
    )
    .bind(u)
    .fetch_one(&pool)
    .await
    .unwrap();
    let pushed = delivery(&pool, just_resolved, "ntfy", None, "retracted", None).await;

    retention::purge(&pool, Some(u)).await.unwrap();

    let kept: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM notification_deliveries WHERE id = $1)")
            .bind(pushed)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(kept, "a push inside the cap window survives the sweep");
    drop_user(&pool, u).await;
}

#[tokio::test]
async fn retention_deletes_acted_notices_past_seven_days() {
    let Some(pool) = db("notify_retention_acted").await else { return };
    let u = user(&pool).await;
    let insert = "INSERT INTO notifications (user_id, kind, title, state, acted_at) \
         VALUES ($1, 'test', 't', 'acted', now() - make_interval(days => $2)) RETURNING id";
    let old: Uuid = sqlx::query_scalar(insert).bind(u).bind(8).fetch_one(&pool).await.unwrap();
    let fresh: Uuid = sqlx::query_scalar(insert).bind(u).bind(1).fetch_one(&pool).await.unwrap();

    retention::purge(&pool, Some(u)).await.unwrap();

    let left: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM notifications WHERE user_id = $1")
        .bind(u)
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(left, vec![fresh]);
    assert!(!left.contains(&old));
    drop_user(&pool, u).await;
}

#[tokio::test]
async fn retention_deletes_only_terminal_rows_past_seven_days() {
    let Some(pool) = db("notify_retention").await else { return };
    let u = user(&pool).await;
    let old_resolved = aged(&pool, u, "resolved", 8).await;
    let old_expired = aged(&pool, u, "expired", 8).await;
    let fresh_resolved = aged(&pool, u, "resolved", 1).await;
    let old_open = aged(&pool, u, "delivered", 8).await;
    let idle = delivery(&pool, fresh_resolved, "a", None, "retracted", None).await;
    let retracting = delivery(&pool, fresh_resolved, "b", Some("retract"), "sent", None).await;
    let open_delivery = delivery(&pool, old_open, "a", None, "sent", None).await;
    token(&pool, fresh_resolved).await;

    retention::purge(&pool, Some(u)).await.unwrap();

    let mut left: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM notifications WHERE user_id = $1")
        .bind(u)
        .fetch_all(&pool)
        .await
        .unwrap();
    left.sort();
    let mut kept = vec![fresh_resolved, old_open];
    kept.sort();
    assert_eq!(left, kept);
    assert!(!left.contains(&old_resolved) && !left.contains(&old_expired));

    let deliveries: Vec<Uuid> = sqlx::query_scalar(
        "SELECT d.id FROM notification_deliveries d JOIN notifications n \
         ON n.id = d.notification_id WHERE n.user_id = $1 ORDER BY d.id",
    )
    .bind(u)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(!deliveries.contains(&idle), "an idle delivery of a terminal notice is deleted");
    assert!(deliveries.contains(&retracting), "a queued retraction is kept");
    assert!(deliveries.contains(&open_delivery), "an open notice keeps its deliveries");
    let tokens: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM notification_action_tokens WHERE notification_id = $1",
    )
    .bind(fresh_resolved)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(tokens, 0);
    drop_user(&pool, u).await;
}
