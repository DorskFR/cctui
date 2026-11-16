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

pub use cctui_proto::api::notifications::{
    Notification as NotificationRecord, NotificationTransition as Transitioned,
};

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
    pub const fn new(user_id: Uuid, kind: &'static str, title: String) -> Self {
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
            actions: serde_json::Value::Array(Vec::new()),
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
    super::in_app::changed(pool, &record).await;
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
    } else if matches!(edge, Edge::Read) {
        cancel_escalations(&mut tx, &[id]).await?;
    }
    tx.commit().await?;
    super::in_app::changed(pool, &notification).await;
    Ok(Some(Transitioned { notification, applied: true }))
}

pub(super) async fn transition_in(
    conn: &mut PgConnection,
    id: Uuid,
    edge: Edge<'_>,
) -> sqlx::Result<Option<NotificationRecord>> {
    let applied = guarded_update(&mut *conn, id, edge).await?;
    if applied.is_some() && edge.target().closes() {
        close_deliveries(conn, id).await?;
    }
    Ok(applied)
}

const LISTED_OPEN_STATES: &[&str] = &["pending", "delivered", "read", "acted"];

#[must_use]
pub fn is_open(n: &NotificationRecord) -> bool {
    OPEN.iter().any(|s| s.as_str() == n.state)
}

async fn cancel_escalations(conn: &mut PgConnection, ids: &[Uuid]) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE notification_deliveries SET state = 'cancelled', op = NULL \
         WHERE notification_id = ANY($1) AND state = 'scheduled' AND stage > 0",
    )
    .bind(ids)
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn unread_count(pool: &PgPool, user_id: Uuid) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT count(*) FROM notifications \
         WHERE user_id = $1 AND state IN ('pending', 'delivered')",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
}

pub async fn get(pool: &PgPool, id: Uuid) -> sqlx::Result<Option<NotificationRecord>> {
    let mut conn = pool.acquire().await?;
    current(&mut conn, id).await
}

pub async fn owned(
    pool: &PgPool,
    user_id: Uuid,
    id: Uuid,
) -> sqlx::Result<Option<NotificationRecord>> {
    sqlx::query_as("SELECT * FROM notifications WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user_id)
        .fetch_optional(pool)
        .await
}

#[derive(Debug, Clone, Copy)]
pub struct Page {
    pub open_only: bool,
    pub limit: i64,
    pub before: Option<DateTime<Utc>>,
}

/// One row past `limit` comes back when there is a next page.
pub async fn list(
    pool: &PgPool,
    user_id: Uuid,
    page: Page,
) -> sqlx::Result<Vec<NotificationRecord>> {
    let states = page.open_only.then(|| LISTED_OPEN_STATES.to_vec());
    sqlx::query_as(
        "SELECT * FROM notifications \
         WHERE user_id = $1 AND ($2::text[] IS NULL OR state = ANY($2)) \
           AND ($3::timestamptz IS NULL OR created_at < $3) \
         ORDER BY created_at DESC, id DESC LIMIT $4",
    )
    .bind(user_id)
    .bind(states)
    .bind(page.before)
    .bind(page.limit + 1)
    .fetch_all(pool)
    .await
}

/// Notices with actions stay unread until answered.
pub async fn read_all(pool: &PgPool, user_id: Uuid) -> sqlx::Result<Vec<NotificationRecord>> {
    let mut tx = pool.begin().await?;
    let read: Vec<NotificationRecord> = sqlx::query_as(
        "UPDATE notifications SET state = 'read', updated_at = now(), \
           read_at = COALESCE(read_at, now()) \
         WHERE user_id = $1 AND state IN ('pending', 'delivered') \
           AND COALESCE(jsonb_array_length(CASE WHEN jsonb_typeof(actions) = 'array' \
                                              THEN actions END), 0) = 0 \
         RETURNING *",
    )
    .bind(user_id)
    .fetch_all(&mut *tx)
    .await?;
    let ids: Vec<Uuid> = read.iter().map(|n| n.id).collect();
    cancel_escalations(&mut tx, &ids).await?;
    tx.commit().await?;
    if !read.is_empty() {
        super::in_app::changed_many(pool, user_id, &read).await;
    }
    Ok(read)
}

#[must_use]
pub fn action<'a>(n: &'a NotificationRecord, id: &str) -> Option<&'a serde_json::Value> {
    n.actions
        .as_array()?
        .iter()
        .find(|a| a.get("id").and_then(serde_json::Value::as_str) == Some(id))
}

#[must_use]
pub fn action_ids(n: &NotificationRecord) -> Vec<&str> {
    n.actions
        .as_array()
        .map(|all| all.iter().filter_map(|a| a.get("id")?.as_str()).collect())
        .unwrap_or_default()
}

/// An approval, or a notice about a blocking user action, holds a run until answered.
pub async fn is_blocking(pool: &PgPool, n: &NotificationRecord) -> sqlx::Result<bool> {
    if n.kind == "approval" {
        return Ok(true);
    }
    if n.subject_kind.as_deref() != Some("user_action") {
        return Ok(false);
    }
    let Some(action_id) = n.subject_id.as_deref() else { return Ok(false) };
    let blocking: Option<bool> =
        sqlx::query_scalar("SELECT blocking FROM session_user_actions WHERE id::text = $1")
            .bind(action_id)
            .fetch_optional(pool)
            .await?;
    Ok(blocking.unwrap_or(false))
}
