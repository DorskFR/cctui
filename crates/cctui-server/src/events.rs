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

use crate::state::AppState;

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
pub fn record(state: &AppState, event: Event) {
    let state = state.clone();
    tokio::spawn(async move {
        if let Err(err) = record_now(&state, event).await {
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
pub async fn record_now(state: &AppState, mut event: Event) -> Result<EventRecord, sqlx::Error> {
    enrich(&state.pool, &mut event).await;
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
    .fetch_one(&state.pool)
    .await?;
    tracing::debug!(id = row.id, kind = %event.kind, %summary, "lifecycle event recorded");
    state.bus.publish_server(cctui_proto::ws::ServerEvent::Event { event: row.clone() });
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

#[cfg(test)]
pub mod tests {
    use super::*;

    fn ev(kind: &str) -> Event {
        Event::new(kind, Actor::System)
    }

    #[test]
    fn session_summaries_prefer_the_name_and_fall_back_to_the_short_id() {
        let named = ev(kind::SESSION_KILLED)
            .session("0123456789abcdef")
            .detail(json!({ "session_name": "deploy prod" }));
        assert_eq!(render_summary(kind::SESSION_KILLED, &named), "deploy prod killed");
        let anon = ev(kind::SESSION_KILLED).session("0123456789abcdef");
        assert_eq!(render_summary(kind::SESSION_KILLED, &anon), "01234567 killed");
    }

    #[test]
    fn ended_carries_the_reason_and_the_first_detail_line() {
        let e = ev(kind::SESSION_ENDED).session("s").detail(json!({
            "session_name": "x",
            "end_reason": "crashed",
            "end_detail": "exit 1\nstderr tail",
        }));
        assert_eq!(render_summary(kind::SESSION_ENDED, &e), "x ended (crashed): exit 1");
    }

    #[test]
    fn machine_offline_counts_the_sessions_it_took_down() {
        let id = Uuid::from_u128(1);
        let e = ev(kind::MACHINE_OFFLINE)
            .machine(id)
            .detail(json!({ "machine_label": "nas", "ended_sessions": 4 }));
        assert_eq!(render_summary(kind::MACHINE_OFFLINE, &e), "nas offline, 4 sessions ended");
        let quiet = ev(kind::MACHINE_OFFLINE).machine(id).detail(json!({ "machine_label": "nas" }));
        assert_eq!(render_summary(kind::MACHINE_OFFLINE, &quiet), "nas offline");
    }

    #[test]
    fn auto_resume_distinguishes_a_nudge_from_exhaustion() {
        let nudge = ev(kind::SESSION_AUTO_RESUMED)
            .session("abcdefgh1")
            .detail(json!({ "attempt": 2, "max_attempts": 3 }));
        assert_eq!(
            render_summary(kind::SESSION_AUTO_RESUMED, &nudge),
            "abcdefgh auto-resume nudge 2/3"
        );
        let done = ev(kind::SESSION_AUTO_RESUMED)
            .session("abcdefgh1")
            .detail(json!({ "exhausted": true, "attempts": 3 }));
        assert_eq!(
            render_summary(kind::SESSION_AUTO_RESUMED, &done),
            "abcdefgh auto-resume gave up after 3 attempts"
        );
    }

    #[test]
    fn unknown_kinds_render_generically_with_their_subject() {
        let e = ev("session.something_new").session("abcdefgh1");
        assert_eq!(render_summary("session.something_new", &e), "abcdefgh session.something_new");
        let m = ev("machine.rebooted").machine(Uuid::from_u128(0xabcd_ef01_2345));
        assert!(render_summary("machine.rebooted", &m).ends_with(" machine.rebooted"));
        let s = ev("system.whatever");
        assert_eq!(render_summary("system.whatever", &s), "system.whatever");
    }

    #[test]
    fn reaper_and_migrations_summaries_read_their_counts() {
        let r = ev(kind::SYSTEM_REAPER_RAN).detail(json!({ "archived": 3, "events_pruned": 10 }));
        assert_eq!(
            render_summary(kind::SYSTEM_REAPER_RAN, &r),
            "reaper archived 3 sessions, pruned 10 events"
        );
        let m = ev(kind::SYSTEM_MIGRATIONS_APPLIED).detail(json!({ "versions": [167, 168] }));
        assert_eq!(
            render_summary(kind::SYSTEM_MIGRATIONS_APPLIED, &m),
            "migrations applied: 167, 168"
        );
    }

    #[test]
    fn detail_merges_objects_and_later_keys_win() {
        let e = ev("x").detail(json!({ "a": 1, "b": 1 })).detail(json!({ "b": 2 }));
        assert_eq!(e.detail, json!({ "a": 1, "b": 2 }));
    }

    #[test]
    fn actors_serialise_to_the_documented_strings() {
        let id = Uuid::from_u128(7);
        assert_eq!(Actor::User(id).to_string(), format!("user:{id}"));
        assert_eq!(Actor::Daemon.to_string(), "daemon");
        assert_eq!(Actor::Reaper.to_string(), "reaper");
        assert_eq!(Actor::System.to_string(), "system");
        assert_eq!(Actor::Agent("s1".into()).to_string(), "agent:s1");
    }

    #[test]
    fn system_kinds_are_the_admin_only_family() {
        assert!(is_system_kind(kind::SYSTEM_SERVER_STARTED));
        assert!(!is_system_kind(kind::SESSION_ENDED));
        assert!(!is_system_kind(kind::MACHINE_OFFLINE));
    }

    #[test]
    fn severity_round_trips() {
        for s in [Severity::Info, Severity::Warn, Severity::Error] {
            assert_eq!(Severity::parse(s.as_str()), Some(s));
        }
        assert_eq!(Severity::parse("loud"), None);
    }

    async fn test_pool(test_name: &str) -> Option<sqlx::PgPool> {
        let url = crate::routes::gateway::test_db_url(test_name)?;
        Some(
            sqlx::postgres::PgPoolOptions::new()
                .max_connections(2)
                .connect(&url)
                .await
                .expect("connect test db"),
        )
    }

    pub async fn seed_user_machine(pool: &sqlx::PgPool, tag: &str) -> (Uuid, Uuid) {
        let uid = Uuid::new_v4();
        let machine = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("{tag}-{uid}"))
            .bind(format!("kh-{tag}-{uid}"))
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO machines (id, user_id, name, key_hash, display_name) \
             VALUES ($1, $2, 'host', $3, 'Lab box')",
        )
        .bind(machine)
        .bind(uid)
        .bind(format!("mk-{tag}-{machine}"))
        .execute(pool)
        .await
        .unwrap();
        (uid, machine)
    }

    pub async fn seed_session(
        pool: &sqlx::PgPool,
        uid: Uuid,
        machine: Uuid,
        name: Option<&str>,
    ) -> String {
        let sid = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status, \
                                   session_name, adapter_id) \
             VALUES ($1, $2, $2, $3, '/w', 'active', $4, 'claude-code')",
        )
        .bind(&sid)
        .bind(machine)
        .bind(uid)
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
        sid
    }

    #[derive(sqlx::FromRow)]
    pub struct Row {
        pub kind: String,
        pub session_id: Option<String>,
        pub machine_id: Option<Uuid>,
        pub user_id: Option<Uuid>,
        pub actor: String,
        pub summary: String,
        pub detail: Value,
    }

    pub async fn rows_for_session(pool: &sqlx::PgPool, session_id: &str) -> Vec<Row> {
        sqlx::query_as(
            "SELECT kind, session_id, machine_id, user_id, actor, summary, detail FROM events \
             WHERE session_id = $1 OR detail->>'session_id' = $1 ORDER BY id",
        )
        .bind(session_id)
        .fetch_all(pool)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn a_session_event_is_enriched_with_its_owner_machine_and_name() {
        let Some(pool) = test_pool("events_enrich_session").await else { return };
        let (uid, machine) = seed_user_machine(&pool, "ev-enrich").await;
        let sid = seed_session(&pool, uid, machine, Some("deploy prod")).await;
        let state = AppState::for_test(pool.clone());
        record_now(&state, Event::new(kind::SESSION_KILLED, Actor::User(uid)).session(&sid))
            .await
            .expect("insert");
        let rows = rows_for_session(&pool, &sid).await;
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.kind, kind::SESSION_KILLED);
        assert_eq!(row.machine_id, Some(machine));
        assert_eq!(row.user_id, Some(uid));
        assert_eq!(row.actor, format!("user:{uid}"));
        assert_eq!(row.summary, "deploy prod killed");
        assert_eq!(row.detail["machine_label"], "Lab box");
    }

    #[tokio::test]
    async fn deleting_the_session_keeps_the_row_with_a_null_subject() {
        let Some(pool) = test_pool("events_survive_session_delete").await else { return };
        let (uid, machine) = seed_user_machine(&pool, "ev-del").await;
        let sid = seed_session(&pool, uid, machine, Some("gone soon")).await;
        let state = AppState::for_test(pool.clone());
        record_now(&state, Event::new(kind::SESSION_ENDED, Actor::Daemon).session(&sid))
            .await
            .expect("insert");
        sqlx::query("DELETE FROM sessions WHERE id = $1").bind(&sid).execute(&pool).await.unwrap();
        let rows = rows_for_session(&pool, &sid).await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session_id, None);
        assert!(rows[0].summary.starts_with("gone soon ended"));
        assert_eq!(rows[0].detail["session_id"], sid);
    }

    #[tokio::test]
    async fn a_failed_insert_is_reported_by_record_now_and_swallowed_by_record() {
        let Some(pool) = test_pool("events_insert_failure").await else { return };
        let state = AppState::for_test(pool.clone());
        let orphan = Event::new(kind::MACHINE_OFFLINE, Actor::System).machine(Uuid::new_v4());
        assert!(record_now(&state, orphan.clone()).await.is_err());
        record(&state, orphan);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM events WHERE machine_id IS NULL AND kind = $1 AND actor = 'system' AND occurred_at > now() - interval '1 second'")
                .bind(kind::MACHINE_OFFLINE)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count, 0);
    }

    fn user_ctx(uid: Uuid) -> axum::Extension<crate::auth::AuthContext> {
        axum::Extension(crate::auth::AuthContext {
            user_id: uid,
            key_id: uid,
            machine_id: None,
            scopes: [crate::auth::Scope::Read, crate::auth::Scope::Dispatch].into_iter().collect(),
        })
    }

    async fn settle() {
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    }

    fn kinds(rows: &[Row]) -> Vec<&str> {
        rows.iter().map(|r| r.kind.as_str()).collect()
    }

    /// A daemon channel on the bus so dispatch-gated routes go through.
    ///
    /// The receiver is kept so the channel stays open.
    fn fake_daemon(
        state: &AppState,
        machine: Uuid,
    ) -> tokio::sync::mpsc::Receiver<cctui_proto::ws::DaemonFrameDown> {
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        state.bus.register_daemon(machine, Uuid::new_v4(), tx);
        rx
    }

    #[tokio::test]
    async fn kill_records_exactly_one_row_with_the_user_as_actor() {
        use axum::extract::{Path, State};
        let Some(pool) = test_pool("events_kill_once").await else { return };
        let (uid, machine) = seed_user_machine(&pool, "ev-kill").await;
        let sid = seed_session(&pool, uid, machine, Some("ship it")).await;
        let state = AppState::for_test(pool.clone());
        crate::routes::sessions::kill_session(State(state), user_ctx(uid), Path(sid.clone()))
            .await
            .expect("kill");
        settle().await;
        let rows = rows_for_session(&pool, &sid).await;
        assert_eq!(kinds(&rows), vec![kind::SESSION_KILLED]);
        assert_eq!(rows[0].actor, format!("user:{uid}"));
        assert_eq!(rows[0].summary, "ship it killed");
        assert_eq!(rows[0].user_id, Some(uid));
        assert_eq!(rows[0].machine_id, Some(machine));
    }

    #[tokio::test]
    async fn archive_then_unarchive_record_one_row_each() {
        use axum::extract::{Path, Query, State};
        let Some(pool) = test_pool("events_archive_once").await else { return };
        let (uid, machine) = seed_user_machine(&pool, "ev-arch").await;
        let sid = seed_session(&pool, uid, machine, Some("old work")).await;
        let state = AppState::for_test(pool.clone());
        crate::routes::sessions::archive_session(
            State(state.clone()),
            user_ctx(uid),
            Path(sid.clone()),
            Query(crate::routes::sessions::ArchiveQuery::default()),
        )
        .await
        .expect("archive");
        settle().await;
        let rows = rows_for_session(&pool, &sid).await;
        assert_eq!(kinds(&rows), vec![kind::SESSION_ARCHIVED]);
        assert_eq!(rows[0].summary, "old work archived");
        assert_eq!(rows[0].detail["initiator"], "user");

        crate::routes::sessions::unarchive_session(
            State(state.clone()),
            user_ctx(uid),
            Path(sid.clone()),
        )
        .await
        .expect("unarchive");
        crate::routes::sessions::unarchive_session(State(state), user_ctx(uid), Path(sid.clone()))
            .await
            .expect("unarchive again");
        settle().await;
        let rows = rows_for_session(&pool, &sid).await;
        assert_eq!(
            kinds(&rows),
            vec![kind::SESSION_ARCHIVED, kind::SESSION_UNARCHIVED],
            "a second unarchive of an already-live session changes nothing"
        );
    }

    #[tokio::test]
    async fn fork_records_one_row_on_the_parent_naming_the_child() {
        use axum::extract::{Path, State};
        let Some(pool) = test_pool("events_fork_once").await else { return };
        let (uid, machine) = seed_user_machine(&pool, "ev-fork").await;
        let sid = seed_session(&pool, uid, machine, Some("root")).await;
        let state = AppState::for_test(pool.clone());
        let _daemon = fake_daemon(&state, machine);
        let (_, axum::Json(res)) = crate::routes::sessions::fork_session(
            State(state),
            user_ctx(uid),
            Path(sid.clone()),
            axum::Json(cctui_proto::api::ForkRequest::default()),
        )
        .await
        .expect("fork");
        settle().await;
        let rows = rows_for_session(&pool, &sid).await;
        assert_eq!(kinds(&rows), vec![kind::SESSION_FORKED]);
        assert_eq!(rows[0].detail["child_session_id"], res.session_id.expect("claude pre-mints"));
        assert_eq!(rows[0].detail["parent_session_id"], sid);
        assert!(rows[0].summary.starts_with("root forked into "));
    }

    #[tokio::test]
    async fn resume_records_exactly_one_manual_row() {
        use axum::extract::{Path, State};
        let Some(pool) = test_pool("events_resume_once").await else { return };
        let (uid, machine) = seed_user_machine(&pool, "ev-resume").await;
        let sid = seed_session(&pool, uid, machine, Some("paused")).await;
        sqlx::query("UPDATE sessions SET status = 'ended', end_reason = 'completed' WHERE id = $1")
            .bind(&sid)
            .execute(&pool)
            .await
            .unwrap();
        let state = AppState::for_test(pool.clone());
        let _daemon = fake_daemon(&state, machine);
        crate::routes::sessions::resume_session(State(state), user_ctx(uid), Path(sid.clone()))
            .await
            .expect("resume");
        settle().await;
        let rows = rows_for_session(&pool, &sid).await;
        assert_eq!(kinds(&rows), vec![kind::SESSION_RESUMED]);
        assert_eq!(rows[0].detail["origin"], "manual");
        assert_eq!(rows[0].summary, "paused resumed (manual)");
    }

    #[tokio::test]
    async fn retention_prunes_only_past_the_window_and_zero_keeps_everything() {
        let Some(pool) = test_pool("events_retention").await else { return };
        let marker = format!("retention-{}", Uuid::new_v4());
        let seed = |age: &'static str| {
            let pool = pool.clone();
            let marker = marker.clone();
            async move {
                sqlx::query(
                    "INSERT INTO events (occurred_at, kind, actor, summary, detail) \
                     VALUES (now() - $1::interval, 'system.test', 'system', $2, '{}')",
                )
                .bind(age)
                .bind(marker)
                .execute(&pool)
                .await
                .unwrap();
            }
        };
        seed("100 days").await;
        seed("1 day").await;
        let count = || {
            let pool = pool.clone();
            let marker = marker.clone();
            async move {
                sqlx::query_scalar::<_, i64>("SELECT count(*) FROM events WHERE summary = $1")
                    .bind(marker)
                    .fetch_one(&pool)
                    .await
                    .unwrap()
            }
        };
        let set = |days: i64| {
            let pool = pool.clone();
            async move {
                sqlx::query(
                    "INSERT INTO instance_settings (key, value, updated_at) VALUES ($1, $2, now()) \
                     ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
                )
                .bind(RETENTION_KEY)
                .bind(json!(days))
                .execute(&pool)
                .await
                .unwrap();
            }
        };
        set(0).await;
        assert_eq!(retention_days(&pool).await, 0);
        prune(&pool).await;
        assert_eq!(count().await, 2);
        set(90).await;
        prune(&pool).await;
        assert_eq!(count().await, 1);
        sqlx::query("DELETE FROM instance_settings WHERE key = $1")
            .bind(RETENTION_KEY)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(retention_days(&pool).await, DEFAULT_RETENTION_DAYS);
    }
}
