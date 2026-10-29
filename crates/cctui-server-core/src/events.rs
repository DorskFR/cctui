//! Lifecycle event log: session, machine and system transitions.
//!
//! An event describes a change in the existence, ownership or reachability
//! of a session, a machine or a server subsystem: tens per session lifetime,
//! never one per turn. Message-level traffic stays in `stream_events`.
//!
//! Recording is fire-and-forget. [`record`] inserts on a detached task and a
//! failed insert is a warning, never an error on the caller's path. Dedup is
//! the caller's job at the transition point: emit where the state is known to
//! have changed, never from a loop that re-observes a steady state.

use serde_json::{Value, json};
use uuid::Uuid;

use crate::bus::Bus;

/// Stable `<subject>.<verb>` kinds.
///
/// Adding one is one call site: `kind` is a text column and unknown kinds
/// render generically.
pub mod kind {
    pub const SESSION_CREATED: &str = "session.created";
    pub const SESSION_LAUNCHED: &str = "session.launched";
    pub const SESSION_ENDED: &str = "session.ended";
    pub const SESSION_ARCHIVED: &str = "session.archived";
    pub const SESSION_UNARCHIVED: &str = "session.unarchived";
    pub const SESSION_FORKED: &str = "session.forked";
    pub const SESSION_RESUMED: &str = "session.resumed";
    pub const SESSION_AUTO_RESUMED: &str = "session.auto_resumed";
    pub const SESSION_KILLED: &str = "session.killed";
    pub const SESSION_INTERRUPTED: &str = "session.interrupted";
    pub const SESSION_GUARD_TRANSITION: &str = "session.guard_transition";
    pub const SESSION_RENAMED: &str = "session.renamed";
    pub const SESSION_MODEL_CHANGED: &str = "session.model_changed";
    pub const SESSION_ACCOUNT_SWITCHED: &str = "session.account_switched";
    pub const SESSION_DELETED: &str = "session.deleted";
    pub const MACHINE_ENROLLED: &str = "machine.enrolled";
    pub const MACHINE_ONLINE: &str = "machine.online";
    pub const MACHINE_STALE: &str = "machine.stale";
    pub const MACHINE_OFFLINE: &str = "machine.offline";
    pub const MACHINE_DAEMON_CONNECTED: &str = "machine.daemon_connected";
    pub const MACHINE_DAEMON_DISCONNECTED: &str = "machine.daemon_disconnected";
    pub const MACHINE_UPDATED: &str = "machine.updated";
    pub const MACHINE_REVOKED: &str = "machine.revoked";
    pub const MACHINE_DELETED: &str = "machine.deleted";
    pub const SYSTEM_SERVER_STARTED: &str = "system.server_started";
    pub const SYSTEM_MIGRATIONS_APPLIED: &str = "system.migrations_applied";
    pub const SYSTEM_REAPER_RAN: &str = "system.reaper_ran";
    pub const SYSTEM_ACCOUNT_LIMIT_REACHED: &str = "system.account_limit_reached";
    pub const SYSTEM_ACCOUNT_LIMIT_CLEARED: &str = "system.account_limit_cleared";
    pub const SYSTEM_DISPATCHER_ONLINE: &str = "system.dispatcher_online";
    pub const SYSTEM_DISPATCHER_OFFLINE: &str = "system.dispatcher_offline";
}

/// `system.*` kinds are admin-only on every read path.
#[must_use]
pub fn is_system_kind(kind: &str) -> bool {
    kind.starts_with("system.")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Severity {
    #[default]
    Info,
    Warn,
    Error,
}

impl Severity {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }

    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "info" => Some(Self::Info),
            "warn" => Some(Self::Warn),
            "error" => Some(Self::Error),
            _ => None,
        }
    }
}

/// Who caused the transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Actor {
    User(Uuid),
    Daemon,
    Reaper,
    System,
    Agent(String),
}

impl std::fmt::Display for Actor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::User(id) => write!(f, "user:{id}"),
            Self::Daemon => f.write_str("daemon"),
            Self::Reaper => f.write_str("reaper"),
            Self::System => f.write_str("system"),
            Self::Agent(session) => write!(f, "agent:{session}"),
        }
    }
}

/// One lifecycle event to record.
///
/// Subject ids the call site does not know are filled in from the subject row
/// at insert time, as are the denormalised `session_name` / `machine_label`
/// the summary is rendered from.
#[derive(Debug, Clone)]
pub struct Event {
    pub kind: String,
    pub severity: Severity,
    pub session_id: Option<String>,
    pub machine_id: Option<Uuid>,
    pub user_id: Option<Uuid>,
    pub actor: Actor,
    pub detail: Value,
}

impl Event {
    #[must_use]
    pub fn new(kind: &str, actor: Actor) -> Self {
        Self {
            kind: kind.to_owned(),
            severity: Severity::Info,
            session_id: None,
            machine_id: None,
            user_id: None,
            actor,
            detail: json!({}),
        }
    }

    #[must_use]
    pub fn session(mut self, id: impl Into<String>) -> Self {
        self.session_id = Some(id.into());
        self
    }

    #[must_use]
    pub const fn machine(mut self, id: Uuid) -> Self {
        self.machine_id = Some(id);
        self
    }

    #[must_use]
    pub const fn user(mut self, id: Uuid) -> Self {
        self.user_id = Some(id);
        self
    }

    #[must_use]
    pub const fn severity(mut self, severity: Severity) -> Self {
        self.severity = severity;
        self
    }

    /// Merge `detail` (an object) into the payload; later keys win.
    #[must_use]
    pub fn detail(mut self, detail: Value) -> Self {
        merge_into(&mut self.detail, detail);
        self
    }
}

fn merge_into(target: &mut Value, extra: Value) {
    match (target.as_object_mut(), extra) {
        (Some(obj), Value::Object(more)) => obj.extend(more),
        (Some(obj), other) if !other.is_null() => {
            obj.insert("value".to_owned(), other);
        }
        _ => {}
    }
}

fn short_id(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

fn str_of<'a>(detail: &'a Value, key: &str) -> Option<&'a str> {
    detail.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

fn num_of(detail: &Value, key: &str) -> Option<i64> {
    detail.get(key).and_then(Value::as_i64)
}

/// The one-line human summary stored with the row.
///
/// Rendered once, at insert, from the kind and its payload so a row stays
/// readable after its subject is gone. Unknown kinds fall back to
/// `<subject> <kind>`.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn render_summary(kind_name: &str, event: &Event) -> String {
    let d = &event.detail;
    let session = || {
        str_of(d, "session_name")
            .map(str::to_owned)
            .or_else(|| event.session_id.as_deref().map(|id| short_id(id).to_owned()))
            .unwrap_or_else(|| "session".to_owned())
    };
    let machine = || {
        str_of(d, "machine_label")
            .map(str::to_owned)
            .or_else(|| event.machine_id.map(|id| short_id(&id.to_string()).to_owned()))
            .unwrap_or_else(|| "machine".to_owned())
    };
    match kind_name {
        kind::SESSION_CREATED => format!(
            "{} created ({})",
            session(),
            str_of(d, "origin").unwrap_or("daemon-registered")
        ),
        kind::SESSION_LAUNCHED => format!("{} launched from a draft", session()),
        kind::SESSION_ENDED => {
            let reason = str_of(d, "end_reason").unwrap_or("unknown");
            str_of(d, "end_detail").map_or_else(
                || format!("{} ended ({reason})", session()),
                |detail| format!("{} ended ({reason}): {}", session(), first_line(detail)),
            )
        }
        kind::SESSION_ARCHIVED => match num_of(d, "children") {
            Some(n) if n > 0 => format!("{} archived with {n} children", session()),
            _ => format!("{} archived", session()),
        },
        kind::SESSION_UNARCHIVED => format!("{} unarchived", session()),
        kind::SESSION_FORKED => str_of(d, "child_session_id").map_or_else(
            || format!("{} forked", session()),
            |child| format!("{} forked into {}", session(), short_id(child)),
        ),
        kind::SESSION_RESUMED => str_of(d, "origin").map_or_else(
            || format!("{} resumed", session()),
            |origin| format!("{} resumed ({origin})", session()),
        ),
        kind::SESSION_AUTO_RESUMED => {
            if d.get("exhausted").and_then(Value::as_bool) == Some(true) {
                format!(
                    "{} auto-resume gave up after {} attempts",
                    session(),
                    num_of(d, "attempts").unwrap_or(0)
                )
            } else {
                format!(
                    "{} auto-resume nudge {}/{}",
                    session(),
                    num_of(d, "attempt").unwrap_or(0),
                    num_of(d, "max_attempts").unwrap_or(0)
                )
            }
        }
        kind::SESSION_KILLED => format!("{} killed", session()),
        kind::SESSION_INTERRUPTED => format!("{} interrupted", session()),
        kind::SESSION_GUARD_TRANSITION => {
            format!("{} asked to move to step {}", session(), str_of(d, "step").unwrap_or("?"))
        }
        kind::SESSION_RENAMED => format!(
            "{} renamed from \"{}\"",
            str_of(d, "to").unwrap_or("untitled"),
            str_of(d, "from").unwrap_or("untitled")
        ),
        kind::SESSION_MODEL_CHANGED => {
            let mut parts = Vec::new();
            if let Some(model) = str_of(d, "model") {
                parts.push(format!("model {model}"));
            }
            if let Some(effort) = str_of(d, "effort") {
                parts.push(format!("effort {effort}"));
            }
            format!("{} set to {}", session(), parts.join(", "))
        }
        kind::SESSION_ACCOUNT_SWITCHED => {
            format!("{} switched {} account", session(), str_of(d, "family").unwrap_or("its"))
        }
        kind::SESSION_DELETED => format!("{} deleted", session()),
        kind::MACHINE_ENROLLED => format!(
            "{} enrolled ({})",
            machine(),
            str_of(d, "machine_kind").unwrap_or("persistent")
        ),
        kind::MACHINE_ONLINE => format!("{} online", machine()),
        kind::MACHINE_STALE => format!("{} stale: no heartbeat", machine()),
        kind::MACHINE_OFFLINE => match num_of(d, "ended_sessions") {
            Some(n) if n > 0 => format!("{} offline, {n} sessions ended", machine()),
            _ => format!("{} offline", machine()),
        },
        kind::MACHINE_DAEMON_CONNECTED => format!("{} daemon connected", machine()),
        kind::MACHINE_DAEMON_DISCONNECTED => format!("{} daemon disconnected", machine()),
        kind::MACHINE_UPDATED => format!(
            "{} update {} from v{} to v{}",
            machine(),
            str_of(d, "phase").unwrap_or("started"),
            str_of(d, "from_version").unwrap_or("?"),
            str_of(d, "to_version").unwrap_or("?")
        ),
        kind::MACHINE_REVOKED => format!("{} revoked", machine()),
        kind::MACHINE_DELETED => format!("{} deleted", machine()),
        kind::SYSTEM_SERVER_STARTED => {
            format!("server started v{}", str_of(d, "version").unwrap_or("?"))
        }
        kind::SYSTEM_MIGRATIONS_APPLIED => {
            let versions: Vec<String> = d
                .get("versions")
                .and_then(Value::as_array)
                .map(|v| v.iter().filter_map(Value::as_i64).map(|n| n.to_string()).collect())
                .unwrap_or_default();
            format!("migrations applied: {}", versions.join(", "))
        }
        kind::SYSTEM_REAPER_RAN => format!(
            "reaper archived {} sessions, pruned {} events",
            num_of(d, "archived").unwrap_or(0),
            num_of(d, "events_pruned").unwrap_or(0)
        ),
        kind::SYSTEM_ACCOUNT_LIMIT_REACHED => format!(
            "account {} rate-limited; {} blocked",
            str_of(d, "account_name").unwrap_or("?"),
            session()
        ),
        kind::SYSTEM_ACCOUNT_LIMIT_CLEARED => format!("{} soft limit cleared", session()),
        kind::SYSTEM_DISPATCHER_ONLINE => {
            format!("dispatcher {} online", str_of(d, "dispatcher_id").map_or("?", short_id))
        }
        kind::SYSTEM_DISPATCHER_OFFLINE => {
            format!("dispatcher {} offline", str_of(d, "dispatcher_id").map_or("?", short_id))
        }
        other => match (event.session_id.is_some(), event.machine_id.is_some()) {
            (true, _) => format!("{} {other}", session()),
            (false, true) => format!("{} {other}", machine()),
            (false, false) => other.to_owned(),
        },
    }
}

fn first_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default().trim();
    let mut out: String = line.chars().take(120).collect();
    if out.len() < line.len() {
        out.push('…');
    }
    out
}

/// Fire-and-forget: insert on a detached task.
///
/// Never blocks or fails the caller; a failed insert is a warning.
pub fn record(pool: &sqlx::PgPool, bus: &Bus, event: Event) {
    let pool = pool.clone();
    let bus = bus.clone();
    tokio::spawn(async move {
        if let Err(err) = record_now(&pool, &bus, event).await {
            tracing::warn!(%err, "lifecycle event insert failed");
        }
    });
}

pub use cctui_proto::api::events::EventRecord;

/// Insert `event`, publish it as [`ServerEvent::Event`] and return the row.
///
/// The body of [`record`]; exposed so a test can observe the failure the
/// detached path only logs.
///
/// [`ServerEvent::Event`]: cctui_proto::ws::ServerEvent::Event
pub async fn record_now(
    pool: &sqlx::PgPool,
    bus: &Bus,
    mut event: Event,
) -> Result<EventRecord, sqlx::Error> {
    enrich(pool, &mut event).await;
    let summary = render_summary(&event.kind, &event);
    let row: EventRecord = sqlx::query_as(
        "INSERT INTO events (kind, severity, session_id, machine_id, user_id, actor, summary, detail) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
         RETURNING id, occurred_at, kind, severity, session_id, machine_id, user_id, actor, \
                   summary, detail",
    )
    .bind(&event.kind)
    .bind(event.severity.as_str())
    .bind(&event.session_id)
    .bind(event.machine_id)
    .bind(event.user_id)
    .bind(event.actor.to_string())
    .bind(&summary)
    .bind(&event.detail)
    .fetch_one(pool)
    .await?;
    tracing::debug!(id = row.id, kind = %event.kind, %summary, "lifecycle event recorded");
    bus.publish_server(cctui_proto::ws::ServerEvent::Event { event: row.clone() });
    Ok(row)
}

/// Fill the subject ids and labels the call site did not carry.
///
/// Best-effort: a lookup failure leaves the event as given.
async fn enrich(pool: &sqlx::PgPool, event: &mut Event) {
    if let Some(session_id) = event.session_id.clone() {
        let row: Option<(Option<Uuid>, Option<Uuid>, Option<String>)> = sqlx::query_as(
            "SELECT machine_uuid, user_id, session_name FROM sessions WHERE id = $1",
        )
        .bind(&session_id)
        .fetch_optional(pool)
        .await
        .unwrap_or_default();
        if let Some((machine_uuid, user_id, name)) = row {
            event.machine_id = event.machine_id.or(machine_uuid);
            event.user_id = event.user_id.or(user_id);
            if let Some(name) = name.filter(|n| !n.trim().is_empty())
                && str_of(&event.detail, "session_name").is_none()
            {
                merge_into(&mut event.detail, json!({ "session_name": name }));
            }
        }
        merge_into(&mut event.detail, json!({ "session_id": session_id }));
    }
    if let Some(machine_id) = event.machine_id {
        let row: Option<(Uuid, String)> = sqlx::query_as(
            "SELECT user_id, COALESCE(display_name, name) FROM machines WHERE id = $1",
        )
        .bind(machine_id)
        .fetch_optional(pool)
        .await
        .unwrap_or_default();
        if let Some((user_id, label)) = row {
            event.user_id = event.user_id.or(Some(user_id));
            if str_of(&event.detail, "machine_label").is_none() {
                merge_into(&mut event.detail, json!({ "machine_label": label }));
            }
        }
    }
}

/// Instance setting: how many days of events to keep. `0` keeps forever.
pub const RETENTION_KEY: &str = "event_retention_days";
pub const DEFAULT_RETENTION_DAYS: i64 = 90;

pub async fn retention_days(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar::<_, Value>("SELECT value FROM instance_settings WHERE key = $1")
        .bind(RETENTION_KEY)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok())))
        .filter(|days| *days >= 0)
        .unwrap_or(DEFAULT_RETENTION_DAYS)
}

/// Delete rows older than the retention window; the number removed.
pub async fn prune(pool: &sqlx::PgPool) -> u64 {
    let days = retention_days(pool).await;
    if days == 0 {
        return 0;
    }
    match sqlx::query("DELETE FROM events WHERE occurred_at < now() - make_interval(days => $1)")
        .bind(i32::try_from(days).unwrap_or(i32::MAX))
        .execute(pool)
        .await
    {
        Ok(res) => res.rows_affected(),
        Err(err) => {
            tracing::warn!(%err, "event retention prune failed");
            0
        }
    }
}
