use serde_json::{Value, json};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use super::store::{Edge, NotificationRecord, awaits_decision, close_in};
use crate::inbox::{Source, session_lane_key};

pub const DECISION_EVENT: &str = "notification.decision";
const SWEEP_LIMIT: i64 = 100;

pub async fn sweep(pool: &PgPool) {
    match expire_due(pool, None).await {
        Ok(0) => {}
        Ok(n) => tracing::debug!(expired = n, "notifications expired"),
        Err(e) => tracing::warn!("notification expiry sweep failed: {e}"),
    }
}

pub async fn expire_due(pool: &PgPool, user: Option<Uuid>) -> sqlx::Result<usize> {
    let due: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM notifications \
         WHERE state IN ('pending', 'delivered', 'read') AND expires_at <= now() \
           AND ($2::uuid IS NULL OR user_id = $2) \
         ORDER BY expires_at LIMIT $1",
    )
    .bind(SWEEP_LIMIT)
    .bind(user)
    .fetch_all(pool)
    .await?;
    let mut expired = 0;
    for id in due {
        match expire(pool, id).await {
            Ok(true) => expired += 1,
            Ok(false) => {}
            Err(e) => tracing::warn!(notification_id = %id, "notification expiry failed: {e}"),
        }
    }
    Ok(expired)
}

async fn expire(pool: &PgPool, id: Uuid) -> sqlx::Result<bool> {
    let mut tx = pool.begin().await?;
    let Some((n, _)) = close_in(&mut tx, id, Edge::Expire).await? else {
        return Ok(false);
    };
    tx.commit().await?;
    super::in_app::changed(pool, &n).await;
    Ok(true)
}

/// The lane written to, when an answered or expired notice holds a run of a session.
pub(super) async fn record_decision(
    conn: &mut PgConnection,
    n: &NotificationRecord,
) -> sqlx::Result<Option<Uuid>> {
    let Some(session_id) = n.session_id.as_deref() else { return Ok(None) };
    if !matches!(n.state.as_str(), "acted" | "expired") || !awaits_decision(&mut *conn, n).await? {
        return Ok(None);
    }
    insert_decision(conn, n, session_id).await.map(Some)
}

#[must_use]
pub fn decision_payload(n: &NotificationRecord, session_id: &str) -> Value {
    let subject = n.subject_kind.as_ref().map(|kind| json!({ "kind": kind, "id": n.subject_id }));
    json!({
        "kind": DECISION_EVENT,
        "notification_id": n.id,
        "session_id": session_id,
        "subject": subject,
        "decision": n.decision,
        "note": n.decision_note,
        "actor": {
            "channel": n.acted_channel.as_deref().unwrap_or("system"),
            "user_id": n.acted_by,
        },
        "decided_at": n.acted_at.or(n.resolved_at),
        "idempotency_key": n.id,
    })
}

fn decision_body(n: &NotificationRecord) -> String {
    let decision = n.decision.as_deref().unwrap_or("expired");
    format!("{decision}: {}", n.decision_note.as_deref().unwrap_or(&n.title))
}

async fn insert_decision(
    conn: &mut PgConnection,
    n: &NotificationRecord,
    session_id: &str,
) -> sqlx::Result<Uuid> {
    let lane: Uuid = sqlx::query_scalar(
        "INSERT INTO inbox_lanes (user_id, key, session_id) VALUES ($1, $2, $3) \
         ON CONFLICT (user_id, key) DO UPDATE \
         SET session_id = COALESCE(inbox_lanes.session_id, EXCLUDED.session_id) \
         RETURNING id",
    )
    .bind(n.user_id)
    .bind(session_lane_key(session_id))
    .bind(session_id)
    .fetch_one(&mut *conn)
    .await?;
    sqlx::query(
        "INSERT INTO inbox_events \
           (lane_id, user_id, source, source_id, body, payload, parent_session_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) \
         ON CONFLICT (lane_id, source, source_id) DO NOTHING",
    )
    .bind(lane)
    .bind(n.user_id)
    .bind(Source::Approval.as_str())
    .bind(n.id.to_string())
    .bind(decision_body(n))
    .bind(decision_payload(n, session_id))
    .bind(session_id)
    .execute(conn)
    .await?;
    Ok(lane)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use sqlx::PgPool;
    use uuid::Uuid;

    use super::expire_due;
    use crate::inbox::store_tests::seed_session;

    async fn db(name: &str) -> Option<PgPool> {
        let url = crate::routes::gateway::test_db_url(name)?;
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        Some(pool)
    }

    async fn notice(
        pool: &PgPool,
        user: Uuid,
        session: &str,
        kind: &str,
        state: &str,
        actions: Value,
        expires_in_secs: i32,
    ) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO notifications (user_id, kind, session_id, subject_kind, subject_id, \
               title, actions, state, expires_at) \
             VALUES ($1, $6, $2, 'outcome', 'o-1', 'Ship it?', $3, $4, \
               now() + make_interval(secs => $5)) \
             RETURNING id",
        )
        .bind(user)
        .bind(session)
        .bind(actions)
        .bind(state)
        .bind(f64::from(expires_in_secs))
        .bind(kind)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    fn approve_reject() -> Value {
        json!([{ "id": "approve", "label": "Approve" }, { "id": "reject", "label": "Reject" }])
    }

    async fn state(pool: &PgPool, id: Uuid) -> (String, Option<String>) {
        sqlx::query_as("SELECT state, decision FROM notifications WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn decision_events(pool: &PgPool, id: Uuid) -> Vec<(String, String, Value, String)> {
        sqlx::query_as(
            "SELECT e.source, e.body, e.payload, l.key FROM inbox_events e \
             JOIN inbox_lanes l ON l.id = e.lane_id WHERE e.source_id = $1",
        )
        .bind(id.to_string())
        .fetch_all(pool)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn an_expired_approval_moves_and_writes_exactly_one_decision_event() {
        let Some(pool) = db("notify_expiry_approval").await else { return };
        let (u, sid) = seed_session(&pool, "expiry-approval").await;
        let id = notice(&pool, u, &sid, "approval", "delivered", approve_reject(), -60).await;
        let delivery: Uuid = sqlx::query_scalar(
            "INSERT INTO notification_deliveries (notification_id, channel, state, provider_ref) \
             VALUES ($1, 'ntfy', 'sent', '{\"sequence_id\": \"s\"}') RETURNING id",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(expire_due(&pool, Some(u)).await.unwrap(), 1);
        assert_eq!(state(&pool, id).await, ("expired".into(), Some("expired".into())));
        let op: Option<String> =
            sqlx::query_scalar("SELECT op FROM notification_deliveries WHERE id = $1")
                .bind(delivery)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(op.as_deref(), Some("retract"), "the sent push is cleared");

        let events = decision_events(&pool, id).await;
        assert_eq!(events.len(), 1);
        let (source, body, payload, lane) = &events[0];
        assert_eq!((source.as_str(), body.as_str()), ("approval", "expired: Ship it?"));
        assert_eq!(lane, &format!("session:{sid}"));
        assert_eq!(payload["kind"], "notification.decision");
        assert_eq!(payload["decision"], "expired");
        assert_eq!(payload["session_id"], sid.as_str());
        assert_eq!(payload["subject"], json!({ "kind": "outcome", "id": "o-1" }));
        assert_eq!(payload["actor"]["channel"], "system");
        assert_eq!(payload["idempotency_key"], id.to_string());

        assert_eq!(expire_due(&pool, Some(u)).await.unwrap(), 0, "already expired");
        sqlx::query("UPDATE notifications SET state = 'read' WHERE id = $1")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(expire_due(&pool, Some(u)).await.unwrap(), 1);
        assert_eq!(decision_events(&pool, id).await.len(), 1, "no second decision event");
    }

    #[tokio::test]
    async fn only_open_past_due_notices_expire_and_info_ones_write_no_event() {
        let Some(pool) = db("notify_expiry_info").await else { return };
        let (u, sid) = seed_session(&pool, "expiry-info").await;
        let info = notice(&pool, u, &sid, "outcome_result", "pending", json!([]), -60).await;
        let fresh = notice(&pool, u, &sid, "approval", "pending", approve_reject(), 3600).await;
        let acted = notice(&pool, u, &sid, "approval", "acted", approve_reject(), -60).await;

        assert_eq!(expire_due(&pool, Some(u)).await.unwrap(), 1);
        assert_eq!(state(&pool, info).await.0, "expired");
        assert!(decision_events(&pool, info).await.is_empty());
        assert_eq!(state(&pool, fresh).await.0, "pending");
        assert_eq!(state(&pool, acted).await.0, "acted");
        assert!(decision_events(&pool, acted).await.is_empty());
    }

    #[tokio::test]
    async fn an_expired_approval_is_published_and_writes_one_event() {
        use cctui_proto::ws::ServerEvent;
        let Some(pool) = db("notify_expiry_publish").await else { return };
        let mut rx = crate::notify::in_app::test_bus().subscribe_server();
        let (u, sid) = seed_session(&pool, "expiry-publish").await;
        let id = notice(&pool, u, &sid, "approval", "pending", approve_reject(), -60).await;

        assert_eq!(expire_due(&pool, Some(u)).await.unwrap(), 1);
        let mut published = Vec::new();
        while let Ok(frame) = rx.try_recv() {
            if let ServerEvent::Notification { notification } = &*frame.event
                && notification.id == id
            {
                published.push(notification.state.clone());
            }
        }
        assert_eq!(published, ["expired"]);
        assert_eq!(decision_events(&pool, id).await.len(), 1);
    }

    #[tokio::test]
    async fn an_expired_info_notice_with_actions_writes_no_event() {
        let Some(pool) = db("notify_expiry_info_actions").await else { return };
        let (u, sid) = seed_session(&pool, "expiry-info-actions").await;
        let id = notice(&pool, u, &sid, "outcome_result", "pending", approve_reject(), -60).await;
        assert_eq!(expire_due(&pool, Some(u)).await.unwrap(), 1);
        assert!(decision_events(&pool, id).await.is_empty(), "only notices that hold a run");
    }
}
