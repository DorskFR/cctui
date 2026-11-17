use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgExecutor, PgPool};
use uuid::Uuid;

use super::prefs::{self, NotificationSettings, NotificationStage};
use super::quiet;
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
    pub blocking: bool,
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
            blocking: false,
        }
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

/// A collapse re-sends to a channel at most this often.
pub const UPDATE_THROTTLE_SECS: i32 = 60;

#[derive(Debug, Clone, Copy, Default)]
pub struct Routing {
    pub ntfy: bool,
    /// `None` is the instance owner.
    pub env_owner: Option<Uuid>,
}

#[derive(Debug, Clone, Copy)]
struct Route {
    channel: &'static str,
    endpoint_id: Option<Uuid>,
    delay_secs: i32,
    stage: i32,
}

const USER_ENDPOINTS_SQL: &str = "\
    SELECT id FROM notification_endpoints \
    WHERE user_id = $1 AND channel = $2 AND enabled \
      AND config ->> 'source' IS DISTINCT FROM 'env' \
    ORDER BY created_at, id";

impl Routing {
    #[must_use]
    pub const fn from_config(config: &Config) -> Self {
        Self { ntfy: config.ntfy_url.is_some() && config.ntfy_token.is_some(), env_owner: None }
    }

    /// The server's ntfy endpoint belongs to one user only.
    pub async fn owns_env(self, conn: &mut PgConnection, user_id: Uuid) -> sqlx::Result<bool> {
        if let Some(owner) = self.env_owner {
            return Ok(owner == user_id);
        }
        let owner: Option<Uuid> = sqlx::query_scalar(super::OWNER_SQL).fetch_optional(conn).await?;
        Ok(owner == Some(user_id))
    }

    async fn plan(
        self,
        conn: &mut PgConnection,
        n: &NewNotification,
        settings: &NotificationSettings,
    ) -> sqlx::Result<Vec<Route>> {
        let key = prefs::route_key(n.kind, n.blocking);
        let mut routes = Vec::new();
        for &channel in prefs::PUSH_CHANNELS {
            let Some(delay_secs) = prefs::delay_secs(prefs::stage(settings, key, channel)) else {
                continue;
            };
            let stage = i32::from(delay_secs > 0);
            for endpoint in self.endpoints(conn, n.user_id, channel).await? {
                routes.push(Route { channel, endpoint_id: Some(endpoint), delay_secs, stage });
            }
        }
        Ok(routes)
    }

    async fn endpoints(
        self,
        conn: &mut PgConnection,
        user_id: Uuid,
        channel: &str,
    ) -> sqlx::Result<Vec<Uuid>> {
        let mut ids: Vec<Uuid> = sqlx::query_scalar(USER_ENDPOINTS_SQL)
            .bind(user_id)
            .bind(channel)
            .fetch_all(&mut *conn)
            .await?;
        if self.ntfy
            && channel == super::ntfy_channel::ID
            && self.owns_env(&mut *conn, user_id).await?
            && let Some(env) = env_endpoint(conn, user_id, channel).await?
        {
            ids.insert(0, env);
        }
        Ok(ids)
    }
}

/// Without a live socket anywhere the human cannot be looking, so the wait is pointless.
fn presence_shortcut(notice_kind: &str) -> bool {
    [super::kind::APPROVAL, super::kind::SESSION_STUCK].contains(&notice_kind)
}

const WATCHED_SQL: &str = "\
    SELECT EXISTS (SELECT 1 FROM session_viewers \
                   WHERE session_id = $2 AND seen_at > now() - make_interval(secs => $3)) \
        OR EXISTS (SELECT 1 FROM session_viewers v JOIN sessions s ON s.id = v.session_id \
                   WHERE s.user_id = $1 AND v.seen_at > now() - make_interval(secs => $3))";

async fn watched(conn: &mut PgConnection, n: &NewNotification) -> sqlx::Result<bool> {
    sqlx::query_scalar(WATCHED_SQL)
        .bind(n.user_id)
        .bind(n.session_id.as_deref())
        .bind(crate::session_viewers::LIVE_WITHIN_SECS)
        .fetch_one(conn)
        .await
}

/// Pushes are counted per notice over the last hour; a notice that already pushes keeps
/// its rows, so a collapse is never throttled.
const CAP_SQL: &str = "\
    SELECT EXISTS (SELECT 1 FROM notification_deliveries \
                   WHERE notification_id = $2 AND channel <> 'in_app'), \
           (SELECT count(DISTINCT d.notification_id) FROM notification_deliveries d \
            JOIN notifications n ON n.id = d.notification_id \
            WHERE n.user_id = $1 AND d.notification_id <> $2 AND d.channel <> 'in_app' \
              AND d.state IN ('scheduled', 'sent', 'retracted') \
              AND n.created_at > now() - interval '1 hour')";

async fn over_cap(
    conn: &mut PgConnection,
    user_id: Uuid,
    notification_id: Uuid,
    cap: u32,
) -> sqlx::Result<bool> {
    let (pushing, recent): (bool, i64) =
        sqlx::query_as(CAP_SQL).bind(user_id).bind(notification_id).fetch_one(conn).await?;
    Ok(!pushing && recent >= i64::from(cap))
}

async fn shape(
    conn: &mut PgConnection,
    n: &NewNotification,
    settings: &NotificationSettings,
    notification_id: Uuid,
    mut routes: Vec<Route>,
) -> sqlx::Result<(Vec<Route>, bool)> {
    if routes.is_empty() {
        return Ok((routes, false));
    }
    if presence_shortcut(n.kind)
        && routes.iter().any(|r| r.stage > 0)
        && !watched(&mut *conn, n).await?
    {
        for r in routes.iter_mut().filter(|r| r.stage > 0) {
            r.delay_secs = 0;
        }
    }
    if let Some(window) = &settings.quiet_hours
        && quiet::holds(n.severity)
    {
        for r in &mut routes {
            let held = quiet::hold(&mut *conn, window, r.delay_secs).await?;
            if held > r.delay_secs {
                r.delay_secs = held;
                r.stage = r.stage.max(1);
            }
        }
    }
    if over_cap(conn, n.user_id, notification_id, settings.max_push_per_hour).await? {
        return Ok((Vec::new(), true));
    }
    Ok((routes, false))
}

fn throttled_notice(user_id: Uuid, cap: u32) -> NewNotification {
    NewNotification {
        collapse_key: Some(super::kind::THROTTLED.to_owned()),
        body: Some(format!(
            "More than {cap} pushes in the last hour: new notices stay in the app for now."
        )),
        ..NewNotification::new(user_id, super::kind::THROTTLED, "Notifications throttled".into())
    }
}

async fn env_endpoint(
    conn: &mut PgConnection,
    user_id: Uuid,
    channel: &str,
) -> sqlx::Result<Option<Uuid>> {
    let (id, enabled) = seed_env_endpoint(conn, user_id, channel).await?;
    Ok(enabled.then_some(id))
}

pub async fn seed_env_endpoint(
    conn: &mut PgConnection,
    user_id: Uuid,
    channel: &str,
) -> sqlx::Result<(Uuid, bool)> {
    sqlx::query_as(
        "INSERT INTO notification_endpoints (user_id, channel, label, config) \
         VALUES ($1, $2, 'server', '{\"source\": \"env\"}') \
         ON CONFLICT (user_id, channel) WHERE config ->> 'source' = 'env' \
         DO UPDATE SET channel = EXCLUDED.channel \
         RETURNING id, enabled",
    )
    .bind(user_id)
    .bind(channel)
    .fetch_one(conn)
    .await
}

const UPSERT_SQL: &str = "\
    INSERT INTO notifications (user_id, kind, severity, session_id, machine_id, subject_kind, \
        subject_id, collapse_key, title, body, tags, actions, expires_at, state, read_at) \
    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, now() + interval '7 days', \
            $13, CASE WHEN $13 = 'read' THEN now() END) \
    ON CONFLICT (user_id, collapse_key) \
        WHERE collapse_key IS NOT NULL AND state IN ('pending', 'delivered', 'read') \
    DO UPDATE SET repeat_count = notifications.repeat_count + 1, \
        title = EXCLUDED.title, body = EXCLUDED.body, severity = EXCLUDED.severity, \
        tags = EXCLUDED.tags, actions = EXCLUDED.actions, updated_at = now() \
    RETURNING *";

/// A notice the user keeps out of the app is written already read.
async fn upsert(
    conn: &mut PgConnection,
    n: &NewNotification,
    in_app: bool,
) -> sqlx::Result<NotificationRecord> {
    let state = if in_app { State::Pending } else { State::Read };
    sqlx::query_as(UPSERT_SQL)
        .bind(n.user_id)
        .bind(n.kind)
        .bind(n.severity)
        .bind(n.session_id.as_deref())
        .bind(n.machine_id)
        .bind(n.subject_kind.as_deref())
        .bind(n.subject_id.as_deref())
        .bind(n.collapse_key.as_deref())
        .bind(n.title.as_str())
        .bind(n.body.as_deref().map(clip))
        .bind(n.tags.as_deref())
        .bind(&n.actions)
        .bind(state.as_str())
        .fetch_one(conn)
        .await
}

/// A collapse queues an update on an idle sent delivery, throttled from its last push.
/// Pending ops render the latest text when claimed, and an op already in flight is
/// re-issued by the dispatcher, so neither needs touching here.
const UPSERT_DELIVERY_SQL: &str = "\
    INSERT INTO notification_deliveries \
        (notification_id, channel, endpoint_id, op, stage, next_attempt_at) \
    VALUES ($1, $2, $3, 'send', $6, now() + make_interval(secs => $4)) \
    ON CONFLICT (notification_id, channel, endpoint_id) DO UPDATE \
    SET op = 'update', \
        next_attempt_at = GREATEST(now(), \
            notification_deliveries.sent_at + make_interval(secs => $5)) \
    WHERE notification_deliveries.state = 'sent' AND notification_deliveries.op IS NULL";

async fn upsert_delivery(
    conn: &mut PgConnection,
    notification_id: Uuid,
    route: Route,
) -> sqlx::Result<()> {
    sqlx::query(UPSERT_DELIVERY_SQL)
        .bind(notification_id)
        .bind(route.channel)
        .bind(route.endpoint_id)
        .bind(route.delay_secs)
        .bind(UPDATE_THROTTLE_SECS)
        .bind(route.stage)
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
    let settings = prefs::load(&mut tx, n.user_id).await?;
    let key = prefs::route_key(n.kind, n.blocking);
    let in_app = prefs::stage(&settings, key, prefs::channel::IN_APP) != NotificationStage::Off;
    let record = upsert(&mut tx, &n, in_app).await?;
    let planned = routing.plan(&mut tx, &n, &settings).await?;
    let (routes, throttled) = shape(&mut tx, &n, &settings, record.id, planned).await?;
    for route in routes {
        upsert_delivery(&mut tx, record.id, route).await?;
    }
    let notice = if throttled {
        let notice = throttled_notice(n.user_id, settings.max_push_per_hour);
        Some(upsert(&mut tx, &notice, true).await?)
    } else {
        None
    };
    tx.commit().await?;
    super::in_app::changed(pool, &record).await;
    if let Some(notice) = &notice {
        super::in_app::changed(pool, notice).await;
    }
    Ok(record)
}

/// One send to one endpoint, past the user's routing, quiet hours and cap.
pub async fn emit_test(
    pool: &PgPool,
    user_id: Uuid,
    channel: &str,
    endpoint_id: Uuid,
) -> sqlx::Result<(NotificationRecord, Uuid)> {
    let n = NewNotification {
        body: Some("If this reached you, the endpoint works.".into()),
        ..NewNotification::new(user_id, super::kind::TEST, "cctui test notification".into())
    };
    let mut tx = pool.begin().await?;
    let record = upsert(&mut tx, &n, true).await?;
    let delivery: Uuid = sqlx::query_scalar(
        "INSERT INTO notification_deliveries (notification_id, channel, endpoint_id, op) \
         VALUES ($1, $2, $3, 'send') RETURNING id",
    )
    .bind(record.id)
    .bind(channel)
    .bind(endpoint_id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    super::in_app::changed(pool, &record).await;
    Ok((record, delivery))
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

async fn transition_in(
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

pub(super) async fn close_in(
    conn: &mut PgConnection,
    id: Uuid,
    edge: Edge<'_>,
) -> sqlx::Result<Option<(NotificationRecord, Option<Uuid>)>> {
    let Some(n) = transition_in(&mut *conn, id, edge).await? else { return Ok(None) };
    let lane = super::expiry::record_decision(conn, &n).await?;
    Ok(Some((n, lane)))
}

#[derive(Debug, Clone)]
pub struct Acted {
    pub transition: Transitioned,
    pub ticked_session: Option<String>,
    pub decision_lane: Option<Uuid>,
}

impl Acted {
    const fn lost(notification: NotificationRecord) -> Self {
        Self {
            transition: Transitioned { notification, applied: false },
            ticked_session: None,
            decision_lane: None,
        }
    }
}

const LOCK_SUBJECT_SQL: &str = "\
    SELECT ua.id FROM notifications n \
    JOIN session_user_actions ua \
      ON n.subject_kind = 'user_action' AND ua.id::text = n.subject_id \
    WHERE n.id = $1 FOR UPDATE OF ua";

async fn tick_subject(
    conn: &mut PgConnection,
    action_id: Uuid,
    action: &str,
    note: Option<&str>,
) -> sqlx::Result<Option<String>> {
    let status = if matches!(action, "dropped" | "reject") { "dropped" } else { "done" };
    sqlx::query_scalar(
        "UPDATE session_user_actions SET status = $2, note = COALESCE($3, note), \
           resolved_at = now(), resolved_by = 'user' \
         WHERE id = $1 AND status = 'open' RETURNING session_id",
    )
    .bind(action_id)
    .bind(status)
    .bind(note)
    .fetch_optional(conn)
    .await
}

/// Locks the user-action row before the notice: the order a tick takes, so the two never
/// deadlock.
pub async fn act(pool: &PgPool, id: Uuid, edge: Edge<'_>) -> sqlx::Result<Option<Acted>> {
    let mut tx = pool.begin().await?;
    let subject: Option<Uuid> =
        sqlx::query_scalar(LOCK_SUBJECT_SQL).bind(id).fetch_optional(&mut *tx).await?;
    let Some((notification, decision_lane)) = close_in(&mut tx, id, edge).await? else {
        return Ok(current(&mut tx, id).await?.map(Acted::lost));
    };
    let ticked_session = match (edge, subject) {
        (Edge::Act { action, note, .. }, Some(action_id)) => {
            tick_subject(&mut tx, action_id, action, note).await?
        }
        _ => None,
    };
    tx.commit().await?;
    super::in_app::changed(pool, &notification).await;
    Ok(Some(Acted {
        transition: Transitioned { notification, applied: true },
        ticked_session,
        decision_lane,
    }))
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
pub async fn awaits_decision<'e>(
    conn: impl PgExecutor<'e>,
    n: &NotificationRecord,
) -> sqlx::Result<bool> {
    if n.kind == super::kind::APPROVAL {
        return Ok(true);
    }
    if n.subject_kind.as_deref() != Some(super::kind::USER_ACTION) {
        return Ok(false);
    }
    let Some(action_id) = n.subject_id.as_deref() else { return Ok(false) };
    let blocking: Option<bool> =
        sqlx::query_scalar("SELECT blocking FROM session_user_actions WHERE id::text = $1")
            .bind(action_id)
            .fetch_optional(conn)
            .await?;
    Ok(blocking.unwrap_or(false))
}

#[cfg(test)]
#[path = "routing_tests.rs"]
mod routing_tests;
