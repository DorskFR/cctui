use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::config::Config;

pub const BODY_MAX_CHARS: usize = 280;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Pending,
    Delivered,
    Read,
    Acted,
    Resolved,
    Expired,
}

impl State {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Delivered => "delivered",
            Self::Read => "read",
            Self::Acted => "acted",
            Self::Resolved => "resolved",
            Self::Expired => "expired",
        }
    }

    #[must_use]
    pub const fn closes(self) -> bool {
        matches!(self, Self::Acted | Self::Resolved | Self::Expired)
    }
}

const OPEN: &[State] = &[State::Pending, State::Delivered, State::Read];

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, Copy)]
pub enum Edge<'a> {
    Deliver,
    Read,
    Act { action: &'a str, note: Option<&'a str>, channel: &'a str, by: Option<Uuid> },
    Dismiss,
    Resolve { reason: &'a str },
    Expire,
}

impl Edge<'_> {
    #[must_use]
    pub const fn sources(self) -> &'static [State] {
        match self {
            Self::Deliver => &[State::Pending],
            Self::Read => &[State::Pending, State::Delivered],
            Self::Act { .. } | Self::Dismiss | Self::Expire => OPEN,
            Self::Resolve { .. } => &[State::Pending, State::Delivered, State::Read, State::Acted],
        }
    }

    #[must_use]
    pub const fn target(self) -> State {
        match self {
            Self::Deliver => State::Delivered,
            Self::Read => State::Read,
            Self::Act { .. } => State::Acted,
            Self::Dismiss | Self::Resolve { .. } => State::Resolved,
            Self::Expire => State::Expired,
        }
    }
}

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct NotificationRecord {
    pub id: Uuid,
    pub user_id: Uuid,
    pub kind: String,
    pub severity: String,
    pub session_id: Option<String>,
    pub machine_id: Option<Uuid>,
    pub subject_kind: Option<String>,
    pub subject_id: Option<String>,
    pub collapse_key: Option<String>,
    pub title: String,
    pub body: Option<String>,
    pub tags: Option<String>,
    pub actions: serde_json::Value,
    pub state: String,
    pub repeat_count: i32,
    pub decision: Option<String>,
    pub decision_note: Option<String>,
    pub acted_channel: Option<String>,
    pub acted_by: Option<Uuid>,
    pub resolved_reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub read_at: Option<DateTime<Utc>>,
    pub acted_at: Option<DateTime<Utc>>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Transitioned {
    pub notification: NotificationRecord,
    pub applied: bool,
}

#[derive(Debug, Clone)]
pub struct NewNotification {
    pub user_id: Uuid,
    pub kind: &'static str,
    pub severity: &'static str,
    pub session_id: Option<String>,
    pub machine_id: Option<Uuid>,
    pub subject_kind: Option<String>,
    pub subject_id: Option<String>,
    pub collapse_key: Option<String>,
    pub title: String,
    pub body: Option<String>,
    pub tags: Option<String>,
    pub actions: serde_json::Value,
}

impl NewNotification {
    #[must_use]
    pub fn new(user_id: Uuid, kind: &'static str, title: String) -> Self {
        Self {
            user_id,
            kind,
            severity: "info",
            session_id: None,
            machine_id: None,
            subject_kind: None,
            subject_id: None,
            collapse_key: None,
            title,
            body: None,
            tags: None,
            actions: serde_json::Value::Array(vec![]),
        }
    }
}

/// ntfy's 1..=5 priority scale.
#[must_use]
pub const fn severity_for_priority(priority: u8) -> &'static str {
    match priority {
        0 | 1 => "min",
        2 => "low",
        3 => "info",
        4 => "high",
        _ => "critical",
    }
}

#[must_use]
pub fn priority_for_severity(severity: &str) -> u8 {
    match severity {
        "min" => 1,
        "low" => 2,
        "high" => 4,
        "critical" => 5,
        _ => 3,
    }
}

#[must_use]
pub fn clip(body: &str) -> String {
    if body.chars().nth(BODY_MAX_CHARS).is_none() {
        return body.to_owned();
    }
    let cut = body.char_indices().nth(BODY_MAX_CHARS - 1).map_or(body.len(), |(i, _)| i);
    format!("{}…", &body[..cut])
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Routing {
    pub ntfy: bool,
}

impl Routing {
    #[must_use]
    pub const fn from_config(config: &Config) -> Self {
        Self { ntfy: config.ntfy_url.is_some() && config.ntfy_token.is_some() }
    }

    async fn plan(
        self,
        conn: &mut PgConnection,
        n: &NewNotification,
    ) -> sqlx::Result<Vec<(&'static str, Option<Uuid>)>> {
        let mut routes = Vec::new();
        if self.ntfy
            && n.kind == super::kind::LEGACY
            && let Some(endpoint) = env_endpoint(conn, n.user_id, super::ntfy_channel::ID).await?
        {
            routes.push((super::ntfy_channel::ID, Some(endpoint)));
        }
        Ok(routes)
    }
}

async fn env_endpoint(
    conn: &mut PgConnection,
    user_id: Uuid,
    channel: &str,
) -> sqlx::Result<Option<Uuid>> {
    let (id, enabled): (Uuid, bool) = sqlx::query_as(
        "INSERT INTO notification_endpoints (user_id, channel, label, config) \
         VALUES ($1, $2, 'server', '{\"source\": \"env\"}') \
         ON CONFLICT (user_id, channel) WHERE config ->> 'source' = 'env' \
         DO UPDATE SET channel = EXCLUDED.channel \
         RETURNING id, enabled",
    )
    .bind(user_id)
    .bind(channel)
    .fetch_one(conn)
    .await?;
    Ok(enabled.then_some(id))
}

const UPSERT_SQL: &str = "\
    INSERT INTO notifications (user_id, kind, severity, session_id, machine_id, subject_kind, \
        subject_id, collapse_key, title, body, tags, actions, expires_at) \
    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, now() + interval '7 days') \
    ON CONFLICT (user_id, collapse_key) \
        WHERE collapse_key IS NOT NULL AND state IN ('pending', 'delivered', 'read') \
    DO UPDATE SET repeat_count = notifications.repeat_count + 1, \
        title = EXCLUDED.title, body = EXCLUDED.body, severity = EXCLUDED.severity, \
        tags = EXCLUDED.tags, actions = EXCLUDED.actions, updated_at = now() \
    RETURNING *";

async fn upsert(conn: &mut PgConnection, n: NewNotification) -> sqlx::Result<NotificationRecord> {
    sqlx::query_as(UPSERT_SQL)
        .bind(n.user_id)
        .bind(n.kind)
        .bind(n.severity)
        .bind(n.session_id)
        .bind(n.machine_id)
        .bind(n.subject_kind)
        .bind(n.subject_id)
        .bind(n.collapse_key)
        .bind(n.title)
        .bind(n.body.as_deref().map(clip))
        .bind(n.tags)
        .bind(n.actions)
        .fetch_one(conn)
        .await
}

async fn upsert_delivery(
    conn: &mut PgConnection,
    notification_id: Uuid,
    channel: &str,
    endpoint_id: Option<Uuid>,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO notification_deliveries (notification_id, channel, endpoint_id, op) \
         VALUES ($1, $2, $3, 'send') \
         ON CONFLICT (notification_id, channel, endpoint_id) DO UPDATE \
         SET op = 'update', next_attempt_at = now() \
         WHERE notification_deliveries.state = 'sent' AND notification_deliveries.op IS NULL",
    )
    .bind(notification_id)
    .bind(channel)
    .bind(endpoint_id)
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn emit(
    pool: &PgPool,
    routing: Routing,
    n: NewNotification,
) -> sqlx::Result<NotificationRecord> {
    let mut tx = pool.begin().await?;
    let routes = routing.plan(&mut tx, &n).await?;
    let record = upsert(&mut tx, n).await?;
    for (channel, endpoint_id) in routes {
        upsert_delivery(&mut tx, record.id, channel, endpoint_id).await?;
    }
    tx.commit().await?;
    Ok(record)
}

const TRANSITION_SQL: &str = "\
    UPDATE notifications SET state = $2, updated_at = now(), \
        read_at = CASE WHEN $2 IN ('read', 'acted') THEN COALESCE(read_at, now()) \
                       ELSE read_at END, \
        acted_at = CASE WHEN $2 = 'acted' THEN now() ELSE acted_at END, \
        resolved_at = CASE WHEN $2 IN ('resolved', 'expired') THEN now() ELSE resolved_at END, \
        decision = COALESCE($4, decision), \
        decision_note = COALESCE($5, decision_note), \
        acted_channel = COALESCE($6, acted_channel), \
        acted_by = COALESCE($7, acted_by), \
        resolved_reason = COALESCE($8, resolved_reason) \
    WHERE id = $1 AND state = ANY($3) \
    RETURNING *";

async fn guarded_update(
    conn: &mut PgConnection,
    id: Uuid,
    edge: Edge<'_>,
) -> sqlx::Result<Option<NotificationRecord>> {
    let (decision, note, channel, by, reason) = match edge {
        Edge::Act { action, note, channel, by } => (Some(action), note, Some(channel), by, None),
        Edge::Dismiss => (Some("dismissed"), None, None, None, Some("dismissed")),
        Edge::Resolve { reason } => (None, None, None, None, Some(reason)),
        Edge::Expire => (Some("expired"), None, None, None, Some("expired")),
        Edge::Deliver | Edge::Read => (None, None, None, None, None),
    };
    let from: Vec<&str> = edge.sources().iter().copied().map(State::as_str).collect();
    sqlx::query_as(TRANSITION_SQL)
        .bind(id)
        .bind(edge.target().as_str())
        .bind(from)
        .bind(decision)
        .bind(note)
        .bind(channel)
        .bind(by)
        .bind(reason)
        .fetch_optional(conn)
        .await
}

async fn close_deliveries(conn: &mut PgConnection, id: Uuid) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE notification_deliveries SET \
           op = CASE WHEN state = 'sent' THEN 'retract' END, \
           state = CASE WHEN state = 'scheduled' THEN 'cancelled' ELSE state END, \
           attempts = 0, next_attempt_at = now() \
         WHERE notification_id = $1 AND state IN ('scheduled', 'sent')",
    )
    .bind(id)
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "UPDATE notification_action_tokens SET used_at = now() \
         WHERE notification_id = $1 AND used_at IS NULL",
    )
    .bind(id)
    .execute(conn)
    .await?;
    Ok(())
}

async fn current(conn: &mut PgConnection, id: Uuid) -> sqlx::Result<Option<NotificationRecord>> {
    sqlx::query_as("SELECT * FROM notifications WHERE id = $1").bind(id).fetch_optional(conn).await
}

/// `applied: false` carries the row as the winner left it. `None` when the id is unknown.
pub async fn transition(
    pool: &PgPool,
    id: Uuid,
    edge: Edge<'_>,
) -> sqlx::Result<Option<Transitioned>> {
    let mut tx = pool.begin().await?;
    let Some(notification) = guarded_update(&mut tx, id, edge).await? else {
        let now = current(&mut tx, id).await?;
        return Ok(now.map(|notification| Transitioned { notification, applied: false }));
    };
    if edge.target().closes() {
        close_deliveries(&mut tx, id).await?;
    }
    tx.commit().await?;
    Ok(Some(Transitioned { notification, applied: true }))
}
