use std::collections::{HashMap, HashSet};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use chrono::{DateTime, Utc};
use serde::Deserialize;

use cctui_proto::adapter::{RemoveInitiator, SessionChild};
use cctui_proto::api::{
    Label, MessageRequest, RegisterRequest, RegisterResponse, RenameRequest, SessionListItem,
    SessionListResponse, SessionParentStub, SessionResult, SpawnRequest, SpawnResponse,
};
use cctui_proto::classifier::{Bucket, ClassifyInput, PrStatus, PrStatusCache, classify};
use cctui_proto::models::{Attention, Liveness, Session, SessionEndReason, SessionStatus};

use crate::auth::AuthContext;
use crate::error::AppError;
use crate::events::{self, Actor, Event, kind};
use crate::followup::{nests_under_parent, next_session_sql};
use crate::live_sessions::live_sessions_predicate;
use crate::routes::spawn::resolve_owned_machine;
use crate::state::AppState;
use crate::store::sessions::SessionRowStatus;

pub use cctui_proto::api::session_bindings::SessionBinding;

pub async fn register(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<RegisterRequest>,
) -> Result<Json<RegisterResponse>, AppError> {
    let Some(machine_uuid) = ctx.machine_id else {
        return Err(AppError::new(StatusCode::FORBIDDEN, "a machine key is required"));
    };
    // Use Claude's session_id directly — it's our primary key now
    let session_id = req.claude_session_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let now = Utc::now();
    let session = Session {
        id: session_id.clone(),
        parent_id: req.parent_session_id,
        account_id: None,
        machine_id: req.machine_id,
        working_dir: req.working_dir,
        status: SessionStatus::New,
        registered_at: now,
        last_heartbeat: now,
        metadata: req.metadata.unwrap_or_else(|| serde_json::json!({})),
        adapter_id: None,
    };

    // A fresh registration is `new` — it becomes `active` on the first
    // transcript line / turn. Re-registration of an already-known session
    // (e.g. Claude restart) is treated the same way: status=new, let the
    // first activity promote it. Only a row of this user on this machine is
    // updated; anything else, including a row with no owner, is left alone.
    let Some(inserted) =
        crate::store::sessions::upsert_registered(&state.pool, &session, machine_uuid, ctx.user_id)
            .await?
    else {
        return Err(AppError::new(StatusCode::NOT_FOUND, "session not found"));
    };
    if inserted {
        events::record(
            &state,
            Event::new(kind::SESSION_CREATED, Actor::Daemon)
                .session(&session_id)
                .machine(machine_uuid)
                .user(ctx.user_id)
                .detail(serde_json::json!({ "origin": "daemon-registered" })),
        );
    }

    let ws_url = format!(
        "{}/api/v1/stream/{}",
        state.config.external_url.replacen("http://", "ws://", 1).replacen("https://", "wss://", 1),
        session_id
    );
    {
        let mut registry = state.registry.write().await;
        registry.register(session.clone());
    }
    // Create (or reuse, on re-registration) the session's live stream channel
    // in the bus — delivery moved out of the registry handle.
    state.bus.register_session_stream(&session_id);

    tracing::info!(session_id = %session_id, machine = %session.machine_id, "session registered");
    Ok(Json(RegisterResponse { session_id, ws_url }))
}

pub async fn deregister(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<StatusCode, AppError> {
    // Deregister just drops the live handle; the session stays in DB as
    // `inactive` and can be revived by a future turn/message.
    crate::store::sessions::set_inactive(&state.pool, &session_id, false).await?;
    {
        let mut registry = state.registry.write().await;
        registry.deregister(&session_id);
    }
    crate::guard_step::forget(&session_id);
    state.bus.deregister_session_stream(&session_id);
    tracing::info!(session_id = %session_id, "session deregistered");
    Ok(StatusCode::NO_CONTENT)
}

// Per-session ownership is enforced by the `Resource(Session, …)` guard in
// `authz.rs`: the single-object session routes declare that policy and the
// `enforce_route` middleware resolves owner via `machine_uuid -> machines.user_id`
// before the handler runs (404 unknown / 403 cross-user / admin bypass). The
// batch routes below still filter inline (`filter_owned_ids`) because a yes/no
// guard can't express "act only on the ids you own".

/// Resolve the owning user for a batch of session ids in one query, then keep
/// only the ids the caller may act on (admins keep every requested id). Used by
/// the batch archive/pin routes so a caller can't sweep other users' sessions.
async fn filter_owned_ids(
    state: &AppState,
    ctx: &AuthContext,
    ids: &[String],
) -> Result<Vec<String>, sqlx::Error> {
    if ctx.is_admin() {
        return Ok(ids.to_vec());
    }
    crate::store::sessions::visible_session_ids(&state.pool, ids, ctx.user_id).await
}

/// Derived-status thresholds. A session is considered:
/// - `Active` if its last heartbeat is within this window;
/// - `New` if it isn't active but was registered within this window;
/// - `Inactive` otherwise.
const STATUS_WINDOW_SECS: i64 = 5 * 60;

/// Liveness-dot thresholds (heartbeat age). `Active` (green) within the
/// active window, `Stale` (orange) up to the dead window, `Dead` (no dot)
/// beyond it.
pub const LIVENESS_ACTIVE_SECS: i64 = 5 * 60;
pub const LIVENESS_DEAD_SECS: i64 = 60 * 60;

/// Query params for `GET /sessions`. Archived sessions are hidden unless
/// `?include_archived=true`.
#[derive(Debug, Default, Deserialize)]
pub struct ListParams {
    #[serde(default)]
    pub include_archived: bool,
}

#[derive(sqlx::FromRow)]
pub struct DbSession {
    id: String,
    parent_id: Option<String>,
    machine_id: String,
    working_dir: String,
    status: String,
    registered_at: DateTime<Utc>,
    last_heartbeat: DateTime<Utc>,
    metadata: serde_json::Value,
    adapter_id: Option<String>,
    resolved_machine_name: Option<String>,
    resolved_machine_hue: Option<i16>,
    resolved_machine_kind: Option<String>,
    room_id: Option<uuid::Uuid>,
    room_name: Option<String>,
}

pub fn derive_status(registered_at: DateTime<Utc>, last_heartbeat: DateTime<Utc>) -> SessionStatus {
    let now = Utc::now();
    if (now - last_heartbeat).num_seconds() < STATUS_WINDOW_SECS {
        SessionStatus::Active
    } else if (now - registered_at).num_seconds() < STATUS_WINDOW_SECS {
        SessionStatus::New
    } else {
        SessionStatus::Inactive
    }
}

/// Map heartbeat age onto the three-tier liveness dot.
pub fn derive_liveness(last_heartbeat: DateTime<Utc>) -> Liveness {
    let age = (Utc::now() - last_heartbeat).num_seconds();
    if age < LIVENESS_ACTIVE_SECS {
        Liveness::Active
    } else if age < LIVENESS_DEAD_SECS {
        Liveness::Stale
    } else {
        Liveness::Dead
    }
}

/// Resolve the row's status + liveness, honouring sticky terminal states.
pub fn resolve_status_liveness(
    row_status: &str,
    registered_at: DateTime<Utc>,
    last_heartbeat: DateTime<Utc>,
) -> (SessionStatus, Liveness) {
    let sticky = SessionRowStatus::parse(row_status).filter(|s| s.is_sticky());
    sticky.map_or_else(
        || (derive_status(registered_at, last_heartbeat), derive_liveness(last_heartbeat)),
        |s| {
            // Archived keeps its real liveness dot; ended/failed/draft → Dead.
            let liveness = if s == SessionRowStatus::Archived {
                derive_liveness(last_heartbeat)
            } else {
                Liveness::Dead
            };
            (s.to_wire(), liveness)
        },
    )
}

/// Classify a session into its bucket from the persisted signals. `children`
/// are the session's linked-PR references (`sessions.children`); the `Review`
/// bucket arises when one of them has an OPEN PR awaiting attention per the
/// `pr_cache` the GitHub connector maintains (empty cache degrades to no Review).
pub fn bucket_from_signals(
    tempo: Option<&str>,
    agent_state: Option<&str>,
    activity: Option<&str>,
    soft_limit_blocked: Option<&str>,
    children: &[SessionChild],
    pr_cache: &std::collections::HashMap<String, PrStatus<'_>>,
) -> Bucket {
    let input = ClassifyInput {
        tempo,
        state: agent_state,
        activity,
        children,
        q: None,
        soft_limit_blocked,
    };
    classify(&input, pr_cache)
}

/// The attention glyph is just the `Blocked` bucket surfaced as ✋ needs input.
pub const fn attention_from_bucket(bucket: Bucket) -> Option<Attention> {
    match bucket {
        Bucket::Blocked => Some(Attention::NeedsInput),
        _ => None,
    }
}

/// Newest `message` event per listed session that is not a turn annotation or
/// a system marker (queue operations included): those never preview a row.
///
/// `event_type = 'message'` must stay character-identical to the predicate of
/// `idx_stream_events_latest_message` (migration 118). The role filter rides
/// on top as a residual, so the backward walk only steps past the few
/// bookkeeping rows that trail a turn. The per-session `LIMIT 1` is the other
/// half: any shape that groups or sorts the whole fan-out instead reads every
/// message row of every listed session.
///
/// Ships only the first 400 chars of the string `text` (else `content`), twice
/// what [`normalize_last_message`] keeps, plus whether it was cut; never the
/// payload.
const LAST_MESSAGE_SQL: &str = "SELECT s.session_id, left(e.body, 400), length(e.body) > 400, \
            e.created_at \
     FROM unnest($1::text[]) AS s(session_id) \
     JOIN LATERAL ( \
         SELECT COALESCE( \
                    CASE jsonb_typeof(se.payload->'text') \
                        WHEN 'string' THEN se.payload->>'text' END, \
                    CASE jsonb_typeof(se.payload->'content') \
                        WHEN 'string' THEN se.payload->>'content' END) AS body, \
                se.created_at \
         FROM stream_events se \
         WHERE se.session_id = s.session_id AND se.event_type = 'message' \
           AND COALESCE(se.payload->>'role', '') NOT IN ('turn_annotation', 'system_marker') \
         ORDER BY se.created_at DESC \
         LIMIT 1 \
     ) e ON true";

/// Per-(session, model) token totals, maintained by a trigger on
/// `session_token_usage` (migration 132). `model` is stored as '' for NULL.
const SESSION_TOTALS_SQL: &str = "SELECT session_id, NULLIF(model, ''), \
            input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens \
     FROM session_usage_totals WHERE session_id = ANY($1)";

/// Unread messages per listed session, counting at most 100 per session: one
/// past the badge's 99 ceiling, so a never-read session stops at the cap
/// instead of counting its whole history.
const UNREAD_COUNT_SQL: &str = "SELECT s.session_id, u.n \
     FROM unnest($1::text[]) AS s(session_id) \
     LEFT JOIN session_reads sr ON sr.session_id = s.session_id AND sr.user_id = $2 \
     CROSS JOIN LATERAL ( \
         SELECT COUNT(*) AS n FROM ( \
             SELECT 1 FROM stream_events se \
             WHERE se.session_id = s.session_id AND se.event_type = 'message' \
               AND (sr.last_seen_at IS NULL OR se.created_at > sr.last_seen_at) \
             LIMIT 100 \
         ) capped \
     ) u";

/// The two newest usage rows per listed session, `rn` 1 = newest: one short
/// backwards walk of `(session_id, created_at DESC)` per session.
const LAST_TWO_TURNS_SQL: &str = "SELECT s.session_id, u.input_tokens, u.cache_read_tokens, \
            u.cache_creation_tokens, u.created_at, \
            row_number() OVER (PARTITION BY s.session_id ORDER BY u.created_at DESC) AS rn \
     FROM unnest($1::text[]) AS s(session_id) \
     JOIN LATERAL ( \
         SELECT stu.input_tokens, stu.cache_read_tokens, stu.cache_creation_tokens, \
                stu.created_at \
         FROM session_token_usage stu \
         WHERE stu.session_id = s.session_id \
         ORDER BY stu.created_at DESC \
         LIMIT 2 \
     ) u ON true \
     ORDER BY s.session_id, rn";

/// `(session_id, preview, cut, created_at)` from [`LAST_MESSAGE_SQL`].
type LastMessageRow = (String, Option<String>, Option<bool>, DateTime<Utc>);

pub async fn list_sessions(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Query(params): Query<ListParams>,
    req_headers: axum::http::HeaderMap,
) -> Result<axum::response::Response, AppError> {
    let owned_live = owned_live_ids(&state, &ctx).await?;
    let mut with_ts: Vec<(DateTime<Utc>, SessionListItem)> = {
        let registry = state.registry.read().await;
        registry
            .list()
            .into_iter()
            .filter(|handle| {
                owned_live.as_ref().is_none_or(|owned| owned.contains(&handle.session.id))
            })
            .map(|handle| (handle.session.registered_at, live_list_item(handle)))
            .collect()
    };

    // Historical inactive sessions from DB (not currently in the live registry).
    let live_ids: HashSet<String> = with_ts.iter().map(|(_, s)| s.id.clone()).collect();
    let rows = fetch_listed_rows(&state, ctx.owner_filter(), params.include_archived).await?;
    with_ts.extend(
        rows.into_iter()
            .filter(|row| !live_ids.contains(&row.id))
            .map(|row| (row.registered_at, db_list_item(row))),
    );

    let sessions = enrich_and_sort(&state, Some(ctx.user_id), with_ts).await?;
    Ok(crate::http_cache::json_with_etag(
        &req_headers,
        &SessionListResponse { sessions, ..Default::default() },
    ))
}

/// Which live registry entries the caller may see: `None` for admins (all of
/// them). The registry's `machine_id` is freeform (UUID or hostname), so
/// ownership can't be read off the handle directly — resolve it from the DB. A
/// live session with no resolvable owner is EXCLUDED for non-admins rather
/// than leaked.
async fn owned_live_ids(
    state: &AppState,
    ctx: &AuthContext,
) -> Result<Option<HashSet<String>>, AppError> {
    if ctx.is_admin() {
        return Ok(None);
    }
    let live_ids: Vec<String> = {
        let registry = state.registry.read().await;
        registry.list().into_iter().map(|h| h.session.id.clone()).collect()
    };
    let owned =
        crate::store::sessions::visible_session_ids(&state.pool, &live_ids, ctx.user_id).await?;
    Ok(Some(owned.into_iter().collect()))
}

/// Session rows for the list, scoped to `uid` (NULL for admin) via
/// `$1::uuid IS NULL OR m.user_id = $1`. ALL non-archived sessions are always
/// returned (no cap) so live/working sessions are never silently truncated.
/// The LIMIT 25 cap applies only to the archived tail, and only when archived
/// history is requested (the webui paginates the archive list separately).
async fn fetch_listed_rows(
    state: &AppState,
    uid: Option<uuid::Uuid>,
    include_archived: bool,
) -> Result<Vec<DbSession>, AppError> {
    let cols = "s.id, s.parent_id, s.machine_id, s.working_dir, s.status, \
                s.registered_at, s.last_heartbeat, s.metadata, s.adapter_id, \
                COALESCE(m.display_name, m.name) AS resolved_machine_name, \
                m.hue AS resolved_machine_hue, m.kind AS resolved_machine_kind, \
                s.room_id, rm.name AS room_name";
    // Archived rooms are joined too: archiving a room archives its sessions, and
    // they keep their room so the archived view still groups by it.
    let non_archived_query = format!(
        "SELECT {cols} \
         FROM sessions s \
         LEFT JOIN machines m ON m.id = s.machine_uuid \
         LEFT JOIN rooms rm ON rm.id = s.room_id \
         WHERE {} \
         AND ($1::uuid IS NULL OR m.user_id = $1) \
         ORDER BY s.registered_at DESC",
        live_sessions_predicate!("s"),
    );
    let mut rows: Vec<DbSession> = sqlx::query_as(sqlx::AssertSqlSafe(non_archived_query))
        .bind(uid)
        .fetch_all(&state.pool)
        .await?;
    if include_archived {
        let archived_query = format!(
            "SELECT {cols} \
             FROM sessions s \
             LEFT JOIN machines m ON m.id = s.machine_uuid \
             LEFT JOIN rooms rm ON rm.id = s.room_id \
             WHERE s.status = 'archived' \
             AND ($1::uuid IS NULL OR m.user_id = $1) \
             ORDER BY s.registered_at DESC LIMIT 25",
        );
        let archived: Vec<DbSession> = sqlx::query_as(sqlx::AssertSqlSafe(archived_query))
            .bind(uid)
            .fetch_all(&state.pool)
            .await?;
        rows.extend(archived);
    }
    Ok(rows)
}

/// A list item with only the identity, status and machine columns filled in;
/// everything else waits for [`enrich`].
#[allow(clippy::too_many_arguments)]
fn skeleton_list_item(
    id: String,
    parent_id: Option<String>,
    machine_id: String,
    working_dir: String,
    status: SessionStatus,
    liveness: Liveness,
    registered_at: DateTime<Utc>,
    last_heartbeat: DateTime<Utc>,
    metadata: serde_json::Value,
    adapter_id: Option<cctui_proto::adapter::AdapterId>,
) -> SessionListItem {
    SessionListItem {
        id,
        parent_id,
        machine_id,
        working_dir,
        status,
        liveness,
        attention: None,
        bucket: Bucket::Working,
        token_usage: cctui_proto::models::TokenUsage::default(),
        origin: cctui_proto::api::SessionOrigin::from_metadata(&metadata),
        metadata,
        adapter_id,
        machine_name: None,
        machine_hue: None,
        machine_kind: None,
        last_message_text: None,
        last_message_at: None,
        registered_at: Some(registered_at),
        name: None,
        model: None,
        effort: None,
        permission_mode: None,
        auto_approve: false,
        match_snippet: None,
        match_seq: None,
        last_activity_at: None,
        cache_cold: false,
        estimated_burst_tokens: None,
        hibernated: false,
        pinned: false,
        labels: Vec::new(),
        room_id: None,
        room_name: None,
        last_heartbeat: Some(last_heartbeat),
        account_name: None,
        unread_count: 0,
        activity_detail: None,
        last_tool_at: None,
        last_tool_name: None,
        tool_use_count: 0,
        todos: Vec::new(),
        user_actions: None,
        context_counts: None,
        has_token_credentials: false,
        account_traffic_observed: false,
        pr_links: Vec::new(),
        end_reason: None,
        end_detail: None,
        ended_at: None,
        auto_archive_at: None,
        archived_by: None,
        keepalive: None,
        last_keepalive_at: None,
        launch_at: None,
        launch_error: None,
        next_session_id: None,
    }
}

/// A live registry entry: time-derived status, the registry's running token
/// usage, no machine label yet.
fn live_list_item(handle: &crate::registry::SessionHandle) -> SessionListItem {
    let session = &handle.session;
    let mut item = skeleton_list_item(
        session.id.clone(),
        session.parent_id.clone(),
        session.machine_id.clone(),
        session.working_dir.clone(),
        derive_status(session.registered_at, session.last_heartbeat),
        derive_liveness(session.last_heartbeat),
        session.registered_at,
        session.last_heartbeat,
        session.metadata.clone(),
        session.adapter_id.clone(),
    );
    item.token_usage = handle.token_usage.clone();
    item
}

/// A historical DB row: sticky terminal states (archived/ended/failed) are NOT
/// re-derived from heartbeat, everything else is time-based; the machine label
/// comes from the row's join.
fn db_list_item(row: DbSession) -> SessionListItem {
    let (status, liveness) =
        resolve_status_liveness(&row.status, row.registered_at, row.last_heartbeat);
    let mut item = skeleton_list_item(
        row.id,
        row.parent_id,
        row.machine_id,
        row.working_dir,
        status,
        liveness,
        row.registered_at,
        row.last_heartbeat,
        row.metadata,
        row.adapter_id.map(cctui_proto::adapter::AdapterId::new),
    );
    item.machine_name = row.resolved_machine_name;
    item.machine_hue = row.resolved_machine_hue;
    item.machine_kind = row.resolved_machine_kind;
    item.room_id = row.room_id.map(|id| id.to_string());
    item.room_name = row.room_name;
    item
}

/// Cap a raw unread `COUNT(*)` to the badge's display ceiling (99). Negative or
/// overflowing DB values clamp into `0..=99`.
fn cap_unread(n: i64) -> u32 {
    n.clamp(0, 99) as u32
}

/// The model catalog each session's usage is priced against, from the provider
/// row its newest session token binds to. Ungatewayed sessions are absent.
pub async fn session_catalogs(
    state: &AppState,
    session_ids: &[String],
) -> std::collections::HashMap<String, serde_json::Value> {
    sqlx::query_as::<_, (String, Option<serde_json::Value>)>(
        "SELECT DISTINCT ON (st.session_id) st.session_id, ap.models \
         FROM session_tokens st JOIN account_providers ap ON ap.id = st.account_id \
         WHERE st.session_id = ANY($1) \
         ORDER BY st.session_id, st.created_at DESC",
    )
    .bind(session_ids)
    .fetch_all(&state.pool)
    .await
    .unwrap_or_else(|e| {
        tracing::warn!("db error (session catalogs): {e}");
        Vec::new()
    })
    .into_iter()
    .filter_map(|(sid, models)| Some((sid, models?)))
    .collect()
}

/// Shared enrichment for both the sessions list and search: resolve machine
/// names, aggregate token usage, attach last-message text, apply classifier
/// signals + display metadata + the in-memory auto-approve flag, then sort
/// most-recent-first. Returns the finished items ready to serialize.
async fn enrich_and_sort(
    state: &AppState,
    viewer: Option<uuid::Uuid>,
    with_ts: Vec<(DateTime<Utc>, SessionListItem)>,
) -> Result<Vec<SessionListItem>, AppError> {
    let ctx = EnrichContext::fetch(state, viewer, &with_ts).await?;
    Ok(sort_sessions(enrich(with_ts, ctx)))
}

/// A machine's resolved label: `display_name` (operator override) over
/// `name`, plus its hue and kind.
type MachineLabel = (String, Option<i16>, String);

/// `(session_id, model, input, output, cache_read, cache_creation)` from
/// [`SESSION_TOTALS_SQL`].
type TokenTotalsRow = (String, Option<String>, Option<i64>, Option<i64>, Option<i64>, Option<i64>);

/// Status signals + display metadata from the session row. A struct, not a
/// tuple: sqlx only implements `FromRow` for tuples up to 16 columns and this
/// select is past that.
#[derive(Debug, Default, sqlx::FromRow)]
struct SignalRow {
    id: String,
    tempo: Option<String>,
    agent_state: Option<String>,
    activity: Option<String>,
    session_name: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    pinned: bool,
    soft_limit_reason: Option<String>,
    last_tool_at: Option<DateTime<Utc>>,
    last_tool_name: Option<String>,
    tool_use_count: i32,
    children: serde_json::Value,
    end_reason: Option<String>,
    end_detail: Option<String>,
    ended_at: Option<DateTime<Utc>>,
    permission_mode: Option<String>,
    archived_by: Option<String>,
    keepalive_json: Option<serde_json::Value>,
    last_keepalive_at: Option<DateTime<Utc>>,
    next_session_id: Option<String>,
}

const SIGNAL_SQL: &str = concat!(
    "SELECT id, tempo, agent_state, activity, session_name, model, effort, pinned, \
            soft_limit_reason, last_tool_at, last_tool_name, tool_use_count, \
            children, end_reason, end_detail, ended_at, permission_mode, archived_by, \
            keepalive_json, last_keepalive_at, ",
    next_session_sql!(),
    " AS next_session_id \
     FROM sessions WHERE id = ANY($1)"
);

/// Everything [`enrich`] merges into the listed items, keyed by session id
/// (machines by `machines.id` or `machines.name`), fetched up front so the
/// merge itself is pure. Sessions absent from a map keep the item's default.
#[derive(Debug, Default)]
struct EnrichContext {
    machines: HashMap<String, MachineLabel>,
    token_usage: HashMap<String, cctui_proto::models::TokenUsage>,
    last_messages: HashMap<String, (Option<String>, DateTime<Utc>)>,
    /// `None` when there is no viewer to count unread messages for.
    unread: Option<HashMap<String, u32>>,
    todos: HashMap<String, serde_json::Value>,
    user_actions: HashMap<String, cctui_proto::api::UserActionCounts>,
    context_counts: HashMap<String, cctui_proto::api::ContextCounts>,
    signals: HashMap<String, SignalRow>,
    pr_snapshot: HashMap<String, cctui_proto::classifier::OwnedPrStatus>,
    labels: HashMap<String, Vec<Label>>,
    account_names: HashMap<String, String>,
    with_credentials: HashSet<String>,
    traffic_observed: HashSet<String>,
    auto_approve: HashSet<String>,
    last_turns: HashMap<String, LastTurn>,
    /// Queued draft launches: `(launch_at, last_error)` by draft id.
    scheduled_launches: HashMap<String, (DateTime<Utc>, Option<String>)>,
    archive_after_secs: u64,
}

impl EnrichContext {
    /// One batched query per signal over the listed sessions; nothing is
    /// queried for an empty list.
    async fn fetch(
        state: &AppState,
        viewer: Option<uuid::Uuid>,
        with_ts: &[(DateTime<Utc>, SessionListItem)],
    ) -> Result<Self, AppError> {
        let session_ids: Vec<String> = with_ts.iter().map(|(_, s)| s.id.clone()).collect();
        let machine_ids: Vec<String> = with_ts
            .iter()
            .map(|(_, s)| s.machine_id.clone())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let mut ctx =
            Self { archive_after_secs: state.config.archive_after_secs, ..Self::default() };
        if !machine_ids.is_empty() {
            ctx.machines = fetch_machine_labels(state, &machine_ids).await?;
        }
        if session_ids.is_empty() {
            return Ok(ctx);
        }
        ctx.token_usage = fetch_token_usage(state, &session_ids).await?;
        ctx.last_messages = fetch_last_messages(state, &session_ids).await?;
        if let Some(uid) = viewer {
            ctx.unread = Some(fetch_unread_counts(state, &session_ids, uid).await?);
        }
        ctx.todos = fetch_todos(state, &session_ids).await?;
        ctx.user_actions = fetch_user_actions(state, &session_ids).await?;
        ctx.context_counts = fetch_context_counts(&state.pool, &session_ids).await?;
        ctx.signals = fetch_signals(state, &session_ids).await?;
        ctx.pr_snapshot = state.pr_status_cache.snapshot();
        ctx.labels = fetch_labels(state, &session_ids).await?;
        ctx.account_names = fetch_account_names(state, &session_ids).await?;
        ctx.with_credentials = fetch_token_holders(state, &session_ids).await?;
        ctx.traffic_observed = fetch_traffic_observed(state, &session_ids).await?;
        ctx.auto_approve = {
            let store = state.permission_store.read().await;
            session_ids.iter().filter(|id| store.is_auto_approve(id)).cloned().collect()
        };
        ctx.last_turns = fetch_last_turns(state, &session_ids).await?;
        let draft_ids: Vec<String> = with_ts
            .iter()
            .filter(|(_, s)| s.status == SessionStatus::Draft)
            .map(|(_, s)| s.id.clone())
            .collect();
        if !draft_ids.is_empty() {
            ctx.scheduled_launches =
                crate::scheduled_spawns::pending_for(&state.pool, &draft_ids).await?;
        }
        Ok(ctx)
    }
}

/// `sessions.machine_id` is freeform: daemon-spawned sessions carry the
/// machine UUID, but legacy `/register` callers carry the OS hostname. Match
/// on either `id` or `name`; historical sessions for purged machines simply
/// stay unresolved.
async fn fetch_machine_labels(
    state: &AppState,
    machine_ids: &[String],
) -> Result<HashMap<String, MachineLabel>, AppError> {
    #[allow(clippy::type_complexity)]
    let rows: Vec<(String, String, Option<String>, Option<i16>, String)> = sqlx::query_as(
        "SELECT id::text, name, display_name, hue, kind FROM machines \
         WHERE id::text = ANY($1) OR name = ANY($1)",
    )
    .bind(machine_ids)
    .fetch_all(&state.pool)
    .await?;
    let mut by_key: HashMap<String, MachineLabel> = HashMap::with_capacity(rows.len() * 2);
    for (id, name, display_name, hue, kind) in rows {
        let resolved = display_name.unwrap_or_else(|| name.clone());
        by_key.insert(id, (resolved.clone(), hue, kind.clone()));
        by_key.insert(name, (resolved, hue, kind));
    }
    Ok(by_key)
}

/// Token usage per session, from the running per-model totals the
/// `session_token_usage` trigger keeps, priced against each session's catalog.
async fn fetch_token_usage(
    state: &AppState,
    session_ids: &[String],
) -> Result<HashMap<String, cctui_proto::models::TokenUsage>, AppError> {
    let rows: Vec<TokenTotalsRow> =
        sqlx::query_as(SESSION_TOTALS_SQL).bind(session_ids).fetch_all(&state.pool).await?;
    let catalogs = session_catalogs(state, session_ids).await;
    Ok(fold_token_totals(rows, &catalogs))
}

/// Sum the per-model totals into one usage per session, pricing each model
/// row against the session's catalog. NULL counters read as 0.
fn fold_token_totals(
    rows: Vec<TokenTotalsRow>,
    catalogs: &HashMap<String, serde_json::Value>,
) -> HashMap<String, cctui_proto::models::TokenUsage> {
    let mut by_session: HashMap<String, cctui_proto::models::TokenUsage> = HashMap::new();
    for (sid, model, ti, to, cr, cc) in rows {
        let (input, cached_input, output, cache_creation) =
            (ti.unwrap_or(0), cr.unwrap_or(0), to.unwrap_or(0), cc.unwrap_or(0));
        let cost = crate::cost::tallies_cost_usd(
            catalogs.get(&sid),
            &[(model, crate::cost::TokenUsage { input, cached_input, output })],
        );
        let to_u64 = |v: i64| u64::try_from(v).unwrap_or(0);
        let e = by_session.entry(sid).or_default();
        e.tokens_in += to_u64(input);
        e.tokens_out += to_u64(output);
        e.cache_read_tokens += to_u64(cached_input);
        e.cache_creation_tokens += to_u64(cache_creation);
        e.cost_usd += cost;
    }
    by_session
}

async fn fetch_last_messages(
    state: &AppState,
    session_ids: &[String],
) -> Result<HashMap<String, (Option<String>, DateTime<Utc>)>, AppError> {
    let rows: Vec<LastMessageRow> =
        sqlx::query_as(LAST_MESSAGE_SQL).bind(session_ids).fetch_all(&state.pool).await?;
    Ok(rows
        .into_iter()
        .map(|(sid, body, cut, ts)| {
            (sid, (body.map(|b| preview_last_message(&b, cut.unwrap_or(false))), ts))
        })
        .collect())
}

/// Unread `message` count per session for the viewer: messages newer than
/// their `session_reads.last_seen_at`, all of them when they have never seen
/// the session. Capped at 99 for the badge.
async fn fetch_unread_counts(
    state: &AppState,
    session_ids: &[String],
    viewer: uuid::Uuid,
) -> Result<HashMap<String, u32>, AppError> {
    let rows: Vec<(String, i64)> = sqlx::query_as(UNREAD_COUNT_SQL)
        .bind(session_ids)
        .bind(viewer)
        .fetch_all(&state.pool)
        .await?;
    Ok(rows.into_iter().map(|(sid, n)| (sid, cap_unread(n))).collect())
}

/// Agent task lists. A separate round-trip rather than another [`SignalRow`]
/// column: that struct's select is already past sqlx's tuple ceiling.
async fn fetch_todos(
    state: &AppState,
    session_ids: &[String],
) -> Result<HashMap<String, serde_json::Value>, AppError> {
    let rows: Vec<(String, serde_json::Value)> =
        sqlx::query_as("SELECT id, todos FROM sessions WHERE id = ANY($1) AND todos IS NOT NULL")
            .bind(session_ids)
            .fetch_all(&state.pool)
            .await?;
    Ok(rows.into_iter().collect())
}

/// Rules the latest launch delivered and memories held, per session; sessions
/// with neither are left out.
async fn fetch_context_counts(
    pool: &sqlx::PgPool,
    session_ids: &[String],
) -> Result<HashMap<String, cctui_proto::api::ContextCounts>, AppError> {
    let rows: Vec<(String, i32, i64)> = sqlx::query_as(
        "SELECT sc.spawn_key, jsonb_array_length(sc.rules), \
         (SELECT count(*) FROM context_items ci \
          WHERE ci.id = ANY(sc.item_ids) AND ci.kind = 'memory') \
         FROM session_context sc WHERE sc.spawn_key = ANY($1)",
    )
    .bind(session_ids)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(sid, rules, memories)| {
            let counts = cctui_proto::api::ContextCounts {
                rules: u32::try_from(rules).unwrap_or(0),
                memories: u32::try_from(memories).unwrap_or(u32::MAX),
            };
            (counts.rules > 0 || counts.memories > 0).then_some((sid, counts))
        })
        .collect())
}

/// Open "needs you" counts, own and rolled up from the session's subagents: a
/// parent following a child has to see that the child is blocked on the user.
async fn fetch_user_actions(
    state: &AppState,
    session_ids: &[String],
) -> Result<HashMap<String, cctui_proto::api::UserActionCounts>, AppError> {
    type CountRow = (String, Option<String>, i64, i64);
    let rows: Vec<CountRow> = sqlx::query_as(concat!(
        "SELECT ua.session_id, CASE WHEN ",
        nests_under_parent!("s"),
        " THEN s.parent_id END AS parent, count(*), \
         count(*) FILTER (WHERE ua.blocking) \
         FROM session_user_actions ua JOIN sessions s ON s.id = ua.session_id \
         WHERE ua.status = 'open' AND (ua.session_id = ANY($1) OR s.parent_id = ANY($1)) \
         GROUP BY 1, 2",
    ))
    .bind(session_ids)
    .fetch_all(&state.pool)
    .await?;
    let listed: HashSet<&String> = session_ids.iter().collect();
    let mut out: HashMap<String, cctui_proto::api::UserActionCounts> = HashMap::new();
    for (sid, parent_id, open, blocking) in &rows {
        let open = u32::try_from(*open).unwrap_or(u32::MAX);
        let blocking = u32::try_from(*blocking).unwrap_or(u32::MAX);
        if listed.contains(sid) {
            let entry = out.entry(sid.clone()).or_default();
            entry.open += open;
            entry.blocking += blocking;
        }
        if let Some(parent) = parent_id.as_ref().filter(|p| listed.contains(p)) {
            let entry = out.entry(parent.clone()).or_default();
            entry.child_open += open;
            entry.child_blocking += blocking;
        }
    }
    Ok(out)
}

async fn fetch_signals(
    state: &AppState,
    session_ids: &[String],
) -> Result<HashMap<String, SignalRow>, AppError> {
    let rows: Vec<SignalRow> =
        sqlx::query_as(SIGNAL_SQL).bind(session_ids).fetch_all(&state.pool).await?;
    Ok(rows.into_iter().map(|row| (row.id.clone(), row)).collect())
}

/// Session labels: one batched join over the junction table, bucketed per
/// session, name-sorted.
async fn fetch_labels(
    state: &AppState,
    session_ids: &[String],
) -> Result<HashMap<String, Vec<Label>>, AppError> {
    type LabelRow = (String, uuid::Uuid, String, String);
    let rows: Vec<LabelRow> = sqlx::query_as(
        "SELECT sl.session_id, l.id, l.name, l.color \
         FROM session_labels sl JOIN labels l ON l.id = sl.label_id \
         WHERE sl.session_id = ANY($1) \
         ORDER BY lower(l.name)",
    )
    .bind(session_ids)
    .fetch_all(&state.pool)
    .await?;
    let mut by_session: HashMap<String, Vec<Label>> = HashMap::new();
    for (sid, id, name, color) in rows {
        by_session.entry(sid).or_default().push(Label { id: id.to_string(), name, color });
    }
    Ok(by_session)
}

/// Account each session runs under. `sessions.account_id` is unused legacy;
/// the real binding lives in `session_tokens` (minted at dispatch/gateway).
/// Resolve the most recent non-revoked token per session to its identity's
/// `accounts.name` (via `account_providers`). Absent for sessions that never
/// routed through the gateway (e.g. plain local sessions).
async fn fetch_account_names(
    state: &AppState,
    session_ids: &[String],
) -> Result<HashMap<String, String>, AppError> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT DISTINCT ON (st.session_id) st.session_id, a.name \
         FROM session_tokens st \
         JOIN account_providers ap ON ap.id = st.account_id \
         JOIN accounts a ON a.id = ap.account_id \
         WHERE st.session_id = ANY($1) AND st.revoked_at IS NULL \
         ORDER BY st.session_id, st.created_at DESC",
    )
    .bind(session_ids)
    .fetch_all(&state.pool)
    .await?;
    Ok(rows.into_iter().collect())
}

/// Sessions holding live token↔account credentials. Independent of the
/// account-name join: a token whose `accounts` row was deleted (or that never
/// resolved a name) still counts. Present iff a non-revoked `session_tokens`
/// row with a present `encrypted_token` exists.
async fn fetch_token_holders(
    state: &AppState,
    session_ids: &[String],
) -> Result<HashSet<String>, AppError> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT DISTINCT session_id FROM session_tokens \
         WHERE session_id = ANY($1) AND revoked_at IS NULL AND encrypted_token IS NOT NULL",
    )
    .bind(session_ids)
    .fetch_all(&state.pool)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Sessions whose live token has actually been presented at the gateway
/// (`last_used_at` stamped). An account-bound session missing here is bound
/// in the DB but never routed through the gateway — the webui's "no gateway
/// traffic observed" warning.
async fn fetch_traffic_observed(
    state: &AppState,
    session_ids: &[String],
) -> Result<HashSet<String>, AppError> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT DISTINCT session_id FROM session_tokens \
         WHERE session_id = ANY($1) AND revoked_at IS NULL AND last_used_at IS NOT NULL",
    )
    .bind(session_ids)
    .fetch_all(&state.pool)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Cold-cache surfacing. The per-message cache split lives in
/// `session_token_usage`; the running totals flatten it, so the two most
/// recent rows per session derive `cache_cold`, `last_activity_at` (lets the
/// client predict cache expiry, Anthropic's ~5-min sliding window, before the
/// next send) and `estimated_burst_tokens`.
async fn fetch_last_turns(
    state: &AppState,
    session_ids: &[String],
) -> Result<HashMap<String, LastTurn>, AppError> {
    let rows: Vec<LastTurnRow> =
        sqlx::query_as(LAST_TWO_TURNS_SQL).bind(session_ids).fetch_all(&state.pool).await?;
    Ok(fold_last_turns(rows))
}

/// Merge the fetched context into the items. Pure: the only inputs are the
/// items and `ctx`.
fn enrich(
    mut with_ts: Vec<(DateTime<Utc>, SessionListItem)>,
    mut ctx: EnrichContext,
) -> Vec<(DateTime<Utc>, SessionListItem)> {
    let pr_cache = PrStatusCache::borrow_map(&ctx.pr_snapshot);
    for (_, s) in &mut with_ts {
        if s.machine_name.is_none()
            && let Some((name, hue, kind)) = ctx.machines.get(&s.machine_id)
        {
            s.machine_name = Some(name.clone());
            s.machine_hue = *hue;
            s.machine_kind = Some(kind.clone());
        }
        if let Some(usage) = ctx.token_usage.remove(&s.id) {
            s.token_usage = usage;
        }
        if let Some((text, ts)) = ctx.last_messages.remove(&s.id) {
            s.last_message_text = text;
            s.last_message_at = Some(ts);
        }
        if let Some(unread) = ctx.unread.as_mut() {
            s.unread_count = unread.remove(&s.id).unwrap_or(0);
        }
        if let Some(v) = ctx.todos.remove(&s.id) {
            s.todos = serde_json::from_value(v).unwrap_or_default();
        }
        s.user_actions = ctx.user_actions.remove(&s.id);
        s.context_counts = ctx.context_counts.remove(&s.id);
        if let Some(row) = ctx.signals.remove(&s.id) {
            apply_signals(s, row, &pr_cache, ctx.archive_after_secs);
        }
        if let Some(labels) = ctx.labels.remove(&s.id) {
            s.labels = labels;
        }
        if let Some(name) = ctx.account_names.remove(&s.id) {
            s.account_name = Some(name);
        }
        s.has_token_credentials = ctx.with_credentials.contains(&s.id);
        s.account_traffic_observed = ctx.traffic_observed.contains(&s.id);
        s.auto_approve = ctx.auto_approve.contains(&s.id);
        if let Some((at, err)) = ctx.scheduled_launches.remove(&s.id) {
            s.launch_at = Some(at);
            s.launch_error = err;
        }
        if let Some(turn) = ctx.last_turns.remove(&s.id) {
            s.last_activity_at = Some(turn.at);
            s.cache_cold = turn.cache_cold();
            s.estimated_burst_tokens = turn.estimated_burst_tokens();
        }
    }
    with_ts
}

/// Apply the session row's status signals + display metadata: the bucket and
/// its ✋ attention glyph (from the classifier), name/model/effort, and the
/// end/archive/keepalive bookkeeping.
fn apply_signals(
    s: &mut SessionListItem,
    row: SignalRow,
    pr_cache: &HashMap<String, PrStatus<'_>>,
    archive_after_secs: u64,
) {
    s.end_reason = row.end_reason.as_deref().map(SessionEndReason::parse);
    s.end_detail = row.end_detail;
    s.ended_at = row.ended_at;
    let children: Vec<SessionChild> = serde_json::from_value(row.children).unwrap_or_default();
    let bucket = bucket_from_signals(
        row.tempo.as_deref(),
        row.agent_state.as_deref(),
        row.activity.as_deref(),
        row.soft_limit_reason.as_deref(),
        &children,
        pr_cache,
    );
    s.pr_links = children.iter().filter(|c| c.kind == "pr").map(|c| c.href.clone()).collect();
    s.bucket = bucket;
    s.attention = attention_from_bucket(bucket);
    // Worker exited but resumable on reply. The adapter parks the marker in
    // `tempo`; the next live snapshot after a resume overwrites it.
    s.hibernated = row.tempo.as_deref() == Some("hibernated");
    s.name = row.session_name;
    s.model = row.model;
    s.effort = row.effort;
    s.permission_mode = row.permission_mode;
    s.pinned = row.pinned;
    s.archived_by = row.archived_by.as_deref().and_then(RemoveInitiator::parse);
    s.auto_archive_at = crate::auto_archive::stale_archive_due(
        s.status,
        s.pinned,
        s.last_heartbeat,
        archive_after_secs,
    );
    s.keepalive = row.keepalive_json.and_then(|v| serde_json::from_value(v).ok());
    s.last_keepalive_at = row.last_keepalive_at;
    s.next_session_id = row.next_session_id;
    s.activity_detail = row.activity;
    s.last_tool_at = row.last_tool_at;
    s.last_tool_name = row.last_tool_name;
    s.tool_use_count = row.tool_use_count.clamp(0, i32::MAX) as u32;
}

/// `(session_id, input, cache_read, cache_creation, created_at, rn)` from
/// [`LAST_TWO_TURNS_SQL`].
type LastTurnRow = (String, i64, i64, i64, DateTime<Utc>, i64);

/// A session's newest usage row, plus the previous turn's context to judge
/// its cache behaviour against (0 when there was no previous turn).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LastTurn {
    cache_read: i64,
    cache_creation: i64,
    at: DateTime<Utc>,
    prev_context: i64,
}

impl LastTurn {
    /// The turn re-billed a prefix it should have read back
    /// (`cache_read < 0.5 * previous context`). Without a previous turn only
    /// the crude `cache_creation > 0 && cache_read == 0` signal is available;
    /// that misses the common case, since tools+system always match, so
    /// `cache_read` is never actually 0.
    fn cache_cold(self) -> bool {
        if self.prev_context > 0 {
            (self.cache_read.max(0) as f64) < (self.prev_context as f64 * 0.5)
        } else {
            self.cache_creation > 0 && self.cache_read == 0
        }
    }

    /// Context size re-written to cache on the next send (≈ the full cached
    /// prefix from the last turn); `None` when nothing was cached.
    fn estimated_burst_tokens(self) -> Option<u64> {
        let burst = u64::try_from(self.cache_read.saturating_add(self.cache_creation)).unwrap_or(0);
        (burst > 0).then_some(burst)
    }
}

/// Fold [`LAST_TWO_TURNS_SQL`] rows (`rn` 1 = newest) into one [`LastTurn`]
/// per session. A `rn = 2` row without its `rn = 1` sibling is ignored.
fn fold_last_turns(rows: Vec<LastTurnRow>) -> HashMap<String, LastTurn> {
    let mut by_session: HashMap<String, LastTurn> = HashMap::new();
    for (sid, input, cache_read, cache_creation, at, rn) in rows {
        match rn {
            1 => {
                by_session.entry(sid).or_insert(LastTurn {
                    cache_read,
                    cache_creation,
                    at,
                    prev_context: 0,
                });
            }
            2 => {
                if let Some(entry) = by_session.get_mut(&sid) {
                    entry.prev_context = input.max(0) + cache_read.max(0) + cache_creation.max(0);
                }
            }
            _ => {}
        }
    }
    by_session
}

/// Pinned sessions sort above everything. Within each group, most recent
/// message first so active sessions float to the top; a session with no
/// messages yet falls back to its registration time.
fn sort_sessions(mut with_ts: Vec<(DateTime<Utc>, SessionListItem)>) -> Vec<SessionListItem> {
    with_ts.sort_by(|a, b| {
        let key = |s: &SessionListItem, reg: DateTime<Utc>| s.last_message_at.unwrap_or(reg);
        b.1.pinned.cmp(&a.1.pinned).then_with(|| key(&b.1, b.0).cmp(&key(&a.1, a.0)))
    });
    with_ts.into_iter().map(|(_, s)| s).collect()
}

/// Query params for `GET /sessions/search`. Each free-text term of `q` is
/// folded like the rendered transcript and matched (case-insensitively,
/// trgm-accelerated) against a session's search chunks plus its
/// id/name/working-dir. `include_archived` sets the scope: `false` searches
/// live (non-archived) sessions only, `true` searches all. With an empty `q`
/// and `include_archived=true` this becomes "browse the archive" — archived
/// sessions, newest first. Pages follow the response's `next_cursor`; `offset`
/// is honoured only without one.
#[derive(Debug, Default, Deserialize)]
pub struct SearchParams {
    #[serde(default)]
    pub q: String,
    #[serde(default)]
    pub include_archived: bool,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub cursor: Option<String>,
}

/// Shared SELECT + machine-name and room joins: the full `DbSession`
/// projection. Every `query_as::<DbSession>` builds on it, so a new field
/// cannot be added to one query and forgotten in another.
pub const DB_SESSION_SELECT: &str = "SELECT s.id, s.parent_id, s.machine_id, s.working_dir, s.status, \
            s.registered_at, s.last_heartbeat, s.metadata, s.adapter_id, \
            COALESCE(m.display_name, m.name) AS resolved_machine_name, \
            m.hue AS resolved_machine_hue, m.kind AS resolved_machine_kind, \
            s.room_id, rm.name AS room_name \
     FROM sessions s \
     LEFT JOIN machines m ON m.id = s.machine_uuid \
     LEFT JOIN rooms rm ON rm.id = s.room_id";

const SEARCH_DEFAULT_LIMIT: i64 = 100;
const SEARCH_MAX_LIMIT: i64 = 500;
/// Sessions tested per statement of the search scan.
const SEARCH_WINDOW: i64 = 200;
/// Past this the scan stops between windows and the page comes back partial.
const SEARCH_BUDGET: std::time::Duration = std::time::Duration::from_secs(3);
/// Caps any one search statement, so a single window cannot outlive the budget
/// by much.
const SEARCH_STATEMENT_TIMEOUT: &str = "SET LOCAL statement_timeout = '5s'";
/// Newest matching chunks of a session ranked for its snippet.
const SNIPPET_CANDIDATES: i64 = 50;

/// Escape LIKE/ILIKE wildcards so a user's literal `%`/`_` aren't treated as
/// pattern metacharacters, then wrap in `%…%` for a substring match. `\` is
/// the default ILIKE escape char.
pub fn ilike_contains(q: &str) -> String {
    let escaped = q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    format!("%{escaped}%")
}

/// [`ilike_contains`] over a free-text term folded like the indexed chunks.
fn text_pattern(term: &str) -> String {
    ilike_contains(&crate::search_index::text::fold(term))
}

/// Free-text terms folded like the indexed chunks; terms that fold to nothing
/// are dropped.
fn fold_terms(terms: &[String]) -> Vec<String> {
    terms.iter().map(|t| crate::search_index::text::fold(t)).filter(|t| !t.is_empty()).collect()
}

fn is_statement_timeout(err: &sqlx::Error) -> bool {
    matches!(err, sqlx::Error::Database(d) if d.code().as_deref() == Some("57014"))
}

/// A positional bind for the compiled query — kept typed so `pinned` binds a
/// real `bool` and every other leaf binds text (raw for `=`, `%…%` for ILIKE).
enum SqlParam {
    Text(String),
    Bool(bool),
}

fn ph_text(params: &mut Vec<SqlParam>, s: String) -> usize {
    params.push(SqlParam::Text(s));
    params.len()
}

/// SQL predicate for one fielded value. The bind is appended to `params` and
/// the returned string references it by its `$N` position (1-based).
fn leaf_predicate(field: &str, value: &str, params: &mut Vec<SqlParam>) -> String {
    match field {
        "machine" => {
            let p = ph_text(params, value.to_string());
            format!(
                "(lower(COALESCE(m.display_name, m.name)) = lower(${p}) \
                  OR lower(s.machine_id) = lower(${p}))"
            )
        }
        "account" => {
            let p = ph_text(params, value.to_string());
            format!(
                "EXISTS (SELECT 1 FROM session_tokens st \
                  JOIN account_providers ap ON ap.id = st.account_id \
                  JOIN accounts a ON a.id = ap.account_id \
                  WHERE st.session_id = s.id AND st.revoked_at IS NULL \
                  AND lower(a.name) = lower(${p}))"
            )
        }
        "tag" => {
            let p = ph_text(params, value.to_string());
            format!(
                "EXISTS (SELECT 1 FROM session_labels sl \
                  JOIN labels l ON l.id = sl.label_id \
                  WHERE sl.session_id = s.id AND lower(l.name) = lower(${p}))"
            )
        }
        "title" => {
            let p = ph_text(params, ilike_contains(value));
            format!("COALESCE(s.session_name, '') ILIKE ${p}")
        }
        "status" => {
            let p = ph_text(params, value.to_string());
            format!("lower(s.status) = lower(${p})")
        }
        "model" => {
            let p = ph_text(params, ilike_contains(value));
            format!("COALESCE(s.model, s.metadata->>'model', '') ILIKE ${p}")
        }
        "effort" => {
            let p = ph_text(params, value.to_string());
            format!("lower(COALESCE(s.effort, '')) = lower(${p})")
        }
        "adapter" => {
            let p = ph_text(params, value.to_string());
            format!("lower(COALESCE(s.adapter_id, '')) = lower(${p})")
        }
        "pinned" => {
            params.push(SqlParam::Bool(value.eq_ignore_ascii_case("true")));
            format!("s.pinned = ${}", params.len())
        }
        "dir" => {
            let p = ph_text(params, ilike_contains(value));
            format!("s.working_dir ILIKE ${p}")
        }
        // Transcript-scoped fields, meaningful only to `/sessions/{id}/search`.
        // Across sessions they stay the literal free text they were before the
        // query registry learned them.
        "role" | "tool" | "after" | "before" => {
            let p = ph_text(params, text_pattern(&format!("{field}:{value}")));
            free_text_predicate(p)
        }
        _ => {
            let p = ph_text(params, text_pattern(value));
            free_text_predicate(p)
        }
    }
}

/// The residual free-text path: id / name / dir / the session's search chunks.
///
/// `(SELECT s.id)` must not be simplified to `s.id`: the wrapper keeps the
/// correlation as a Param, and a plain `s.id` lets the planner hoist the
/// sublink into a hashed subplan that rechecks every matching event in the
/// table rather than probing one session (13 s vs 0.35 s on 600k events).
fn free_text_predicate(p: usize) -> String {
    format!(
        "(s.id ILIKE ${p} OR COALESCE(s.session_name, '') ILIKE ${p} \
          OR s.working_dir ILIKE ${p} \
          OR EXISTS (SELECT 1 FROM search_chunks c \
                     WHERE c.session_id = (SELECT s.id) AND c.body ILIKE ${p}))"
    )
}

fn compile_filter(filter: &cctui_query::Filter, params: &mut Vec<SqlParam>) -> String {
    let preds: Vec<String> =
        filter.values.iter().map(|v| leaf_predicate(&filter.field, v, params)).collect();
    let joined = match preds.len() {
        0 => "TRUE".to_string(),
        1 => preds.into_iter().next().unwrap(),
        _ => format!("({})", preds.join(" OR ")),
    };
    if filter.op == cctui_query::FilterOp::Ne { format!("(NOT {joined})") } else { joined }
}

/// Walk the query AST into a parameterised SQL `WHERE` fragment,
/// pushing binds onto `params` in `$1…$N` order. Fielded leaves become column
/// predicates / join-EXISTS, free text takes the `pg_trgm` path, and the
/// boolean/negation structure maps straight to `AND`/`OR`/`NOT`.
fn compile_node(node: &cctui_query::Node, params: &mut Vec<SqlParam>) -> String {
    compile_node_at(node, params, 0)
}

const MAX_COMPILE_DEPTH: usize = 4 * cctui_query::MAX_DEPTH + 8;

fn compile_node_at(node: &cctui_query::Node, params: &mut Vec<SqlParam>, depth: usize) -> String {
    use cctui_query::Node;
    if depth > MAX_COMPILE_DEPTH {
        return "FALSE".to_string();
    }
    match node {
        Node::Empty => "TRUE".to_string(),
        Node::Text { value } => {
            let p = ph_text(params, text_pattern(value));
            free_text_predicate(p)
        }
        Node::Filter { filter } => compile_filter(filter, params),
        Node::Not { child } => format!("(NOT {})", compile_node_at(child, params, depth + 1)),
        Node::And { children } => join_children(children, "AND", params, depth),
        Node::Or { children } => join_children(children, "OR", params, depth),
    }
}

fn join_children(
    children: &[cctui_query::Node],
    sep: &str,
    params: &mut Vec<SqlParam>,
    depth: usize,
) -> String {
    let preds: Vec<String> =
        children.iter().map(|c| compile_node_at(c, params, depth + 1)).collect();
    if preds.is_empty() {
        "TRUE".to_string()
    } else {
        format!("({})", preds.join(&format!(" {sep} ")))
    }
}

/// 400 for a raw search query too long to parse.
fn check_query_len(q: &str) -> Result<(), AppError> {
    if q.len() > cctui_query::MAX_QUERY_LEN {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            format!("query too long (max {} bytes)", cctui_query::MAX_QUERY_LEN),
        ));
    }
    Ok(())
}

/// The best-ranked of a session's newest chunks matching any term, at most
/// one row per session. `$1` is the session-id array, `$2…$n+1` the ILIKE
/// patterns, `$n+2` the terms the rank weighs.
fn snippet_sql(n_patterns: usize) -> String {
    let or =
        (2..=n_patterns + 1).map(|i| format!("c.body ILIKE ${i}")).collect::<Vec<_>>().join(" OR ");
    let rank = n_patterns + 2;
    format!(
        "SELECT sid, hit.body, hit.event_id FROM unnest($1::text[]) AS sid \
         CROSS JOIN LATERAL ( \
           SELECT m.body, m.event_id FROM ( \
             SELECT c.body, c.event_id FROM search_chunks c \
             WHERE c.session_id = sid AND ({or}) \
             ORDER BY c.event_id DESC LIMIT {SNIPPET_CANDIDATES} \
           ) m \
           ORDER BY ts_rank(to_tsvector('simple', m.body), plainto_tsquery('simple', ${rank})) DESC, \
                    m.event_id DESC \
           LIMIT 1 \
         ) hit"
    )
}

/// Build a ~200-char snippet of `text` centered on the earliest case-insensitive
/// occurrence of any `needle`, so the UI can show why a session matched.
fn make_snippet(text: &str, needles: &[String]) -> String {
    const WINDOW: usize = 200;
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let hay = collapsed.to_lowercase();
    // Earliest byte hit among all needles → char offset to center on.
    let match_byte = needles.iter().filter_map(|n| hay.find(&n.to_lowercase())).min().unwrap_or(0);
    let match_char = collapsed[..match_byte].chars().count();
    let start = match_char.saturating_sub(WINDOW / 2);

    let mut offsets = collapsed.char_indices().map(|(i, _)| i).skip(start);
    let start_byte = offsets.next().unwrap_or(collapsed.len());
    let end_byte = offsets.nth(WINDOW - 1).unwrap_or(collapsed.len());
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.push_str(&collapsed[start_byte..end_byte]);
    if end_byte < collapsed.len() {
        out.push('…');
    }
    out
}

/// `GET /sessions/search?q=…&include_archived=…&limit=…&cursor=…`.
/// Full-transcript search, scoped to live or all sessions; with an empty `q`
/// and `include_archived=true` it browses the archive. Returns the same
/// `SessionListItem` shape as the list (so clients reuse `SessionCard`), each
/// carrying a `match_snippet` when the hit was in the transcript.
pub async fn search_sessions(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Query(params): Query<SearchParams>,
) -> Result<Json<SessionListResponse>, AppError> {
    check_query_len(&params.q)?;
    let uid = ctx.owner_filter();
    // Parse the raw `q` into the AST. A blank query → `Empty` → browse.
    // A plain keyword parses to a single free-text leaf, so back-compat holds.
    let root = cctui_query::parse(params.q.trim());
    let browse = root.is_empty();
    if browse && !params.include_archived {
        return Ok(Json(SessionListResponse::default()));
    }
    let page = SearchPage {
        limit: params.limit.unwrap_or(SEARCH_DEFAULT_LIMIT).clamp(1, SEARCH_MAX_LIMIT),
        offset: params.offset.unwrap_or(0).max(0),
        cursor: params.cursor.as_deref().and_then(parse_search_cursor),
        uid,
    };
    let found = if browse {
        browse_archive(&state.pool, &page).await?
    } else {
        // Live-only scope normally drops archived rows, but a session that's
        // currently live in the in-memory registry (e.g. a dispatched worker
        // auto-archived in the DB while still running) must stay searchable —
        // the bucketed live list surfaces those via the registry, so search
        // would otherwise be the one place they vanish.
        let live_ids: Option<Vec<String>> = if params.include_archived {
            None
        } else {
            let registry = state.registry.read().await;
            Some(registry.list().into_iter().map(|h| h.session.id.clone()).collect())
        };
        scan_matches(&state.pool, &root, live_ids.as_deref(), &page).await?
    };

    let with_ts: Vec<(DateTime<Utc>, SessionListItem)> =
        found.rows.into_iter().map(|row| (row.registered_at, db_list_item(row))).collect();
    let mut sessions = enrich_and_sort(&state, Some(ctx.user_id), with_ts).await?;

    // Attach a transcript snippet per session, windowed around the keyword.
    // Sessions matched only by id/name/dir have no transcript hit and keep
    // `match_snippet = None`.
    let text_terms = fold_terms(&root.free_text_terms());
    let ids: Vec<String> = sessions.iter().map(|s| s.id.clone()).collect();
    if !browse && !ids.is_empty() && !text_terms.is_empty() {
        let sql = snippet_sql(text_terms.len());
        let mut query =
            sqlx::query_as::<_, (String, String, i64)>(sqlx::AssertSqlSafe(sql)).bind(&ids);
        for t in &text_terms {
            query = query.bind(ilike_contains(t));
        }
        let snippet_rows = query.bind(text_terms.join(" ")).fetch_all(&state.pool).await?;
        attach_transcript_hits(&mut sessions, snippet_rows, &text_terms);
    }

    let parents = fetch_parent_stubs(&state.pool, &missing_parent_ids(&sessions), uid).await?;
    Ok(Json(SessionListResponse {
        sessions,
        parents,
        next_cursor: found.next_cursor,
        partial: found.partial,
    }))
}

/// One requested page: size, legacy offset, keyset cursor and owner scope.
struct SearchPage {
    limit: i64,
    offset: i64,
    cursor: Option<(DateTime<Utc>, String)>,
    uid: Option<uuid::Uuid>,
}

struct FoundPage {
    rows: Vec<DbSession>,
    next_cursor: Option<String>,
    partial: bool,
}

/// `<registered_at µs>:<session id>`, the keyset position of a row in the
/// newest-first order.
fn search_cursor(registered_at: DateTime<Utc>, id: &str) -> String {
    format!("{}:{id}", registered_at.timestamp_micros())
}

fn parse_search_cursor(raw: &str) -> Option<(DateTime<Utc>, String)> {
    let (micros, id) = raw.split_once(':')?;
    let at = DateTime::from_timestamp_micros(micros.parse().ok()?)?;
    (!id.is_empty()).then(|| (at, id.to_owned()))
}

/// Archived sessions, newest first. `$3` scopes to the caller (NULL = admin).
async fn browse_archive(pool: &sqlx::PgPool, page: &SearchPage) -> Result<FoundPage, AppError> {
    let sql = format!(
        "{DB_SESSION_SELECT} WHERE s.status = 'archived' \
         AND ($3::uuid IS NULL OR m.user_id = $3) \
         AND ($4::timestamptz IS NULL OR (s.registered_at, s.id) < ($4, $5)) \
         ORDER BY s.registered_at DESC, s.id DESC LIMIT $1 OFFSET $2"
    );
    let offset = if page.cursor.is_some() { 0 } else { page.offset };
    let rows: Vec<DbSession> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(page.limit)
        .bind(offset)
        .bind(page.uid)
        .bind(page.cursor.as_ref().map(|c| c.0))
        .bind(page.cursor.as_ref().map(|c| c.1.as_str()))
        .fetch_all(pool)
        .await?;
    let full = usize::try_from(page.limit).is_ok_and(|l| rows.len() >= l);
    let next_cursor = rows.last().filter(|_| full).map(|r| search_cursor(r.registered_at, &r.id));
    Ok(FoundPage { rows, next_cursor, partial: false })
}

/// The next [`SEARCH_WINDOW`] in-scope sessions after `$2/$3`, newest first.
/// `$1` owner, `$4` window size, `$5` live registry ids when live-only.
fn search_window_sql(live_only: bool) -> String {
    let scope = if live_only {
        format!("({} OR s.id = ANY($5))", live_sessions_predicate!("s"))
    } else {
        "TRUE".to_owned()
    };
    format!(
        "SELECT s.id, s.registered_at FROM sessions s \
         LEFT JOIN machines m ON m.id = s.machine_uuid \
         WHERE ({scope}) AND ($1::uuid IS NULL OR m.user_id = $1) \
           AND ($2::timestamptz IS NULL OR (s.registered_at, s.id) < ($2, $3)) \
         ORDER BY s.registered_at DESC, s.id DESC LIMIT $4"
    )
}

/// Walk in-scope sessions newest first, one window per statement, testing the
/// compiled query against each window until the page is full, the sessions
/// run out, or [`SEARCH_BUDGET`] is spent; the last returns what was found
/// with `partial` set and a cursor to resume from.
async fn scan_matches(
    pool: &sqlx::PgPool,
    root: &cctui_query::Node,
    live_ids: Option<&[String]>,
    page: &SearchPage,
) -> Result<FoundPage, AppError> {
    let mut ast_params: Vec<SqlParam> = Vec::new();
    let where_sql = compile_node(root, &mut ast_params);
    let match_sql = format!(
        "{DB_SESSION_SELECT} WHERE s.id = ANY(${}) AND ({where_sql}) \
         ORDER BY s.registered_at DESC, s.id DESC",
        ast_params.len() + 1
    );
    let window_sql = search_window_sql(live_ids.is_some());
    let want = usize::try_from(page.limit).unwrap_or(1);
    let mut skip =
        if page.cursor.is_some() { 0 } else { usize::try_from(page.offset).unwrap_or(0) };
    let mut cursor = page.cursor.clone();
    let deadline = std::time::Instant::now() + SEARCH_BUDGET;
    let mut rows: Vec<DbSession> = Vec::new();
    let mut tx = pool.begin().await?;
    sqlx::query(SEARCH_STATEMENT_TIMEOUT).execute(&mut *tx).await?;
    let partial = |rows, cursor: Option<(DateTime<Utc>, String)>| FoundPage {
        rows,
        next_cursor: cursor.map(|(at, id)| search_cursor(at, &id)),
        partial: true,
    };
    loop {
        let mut wq =
            sqlx::query_as::<_, (String, DateTime<Utc>)>(sqlx::AssertSqlSafe(window_sql.clone()))
                .bind(page.uid)
                .bind(cursor.as_ref().map(|c| c.0))
                .bind(cursor.as_ref().map(|c| c.1.clone()))
                .bind(SEARCH_WINDOW);
        if let Some(live) = live_ids {
            wq = wq.bind(live);
        }
        let window = match wq.fetch_all(&mut *tx).await {
            Ok(w) => w,
            Err(e) if is_statement_timeout(&e) => return Ok(partial(rows, cursor)),
            Err(e) => return Err(e.into()),
        };
        let Some(last) = window.last().map(|(id, at)| (*at, id.clone())) else {
            return Ok(FoundPage { rows, next_cursor: None, partial: false });
        };
        let exhausted = usize::try_from(SEARCH_WINDOW).is_ok_and(|w| window.len() < w);
        let ids: Vec<String> = window.into_iter().map(|(id, _)| id).collect();
        let mut mq = sqlx::query_as::<_, DbSession>(sqlx::AssertSqlSafe(match_sql.clone()));
        for p in &ast_params {
            mq = match p {
                SqlParam::Text(s) => mq.bind(s.clone()),
                SqlParam::Bool(b) => mq.bind(*b),
            };
        }
        let found = match mq.bind(&ids).fetch_all(&mut *tx).await {
            Ok(f) => f,
            Err(e) if is_statement_timeout(&e) => return Ok(partial(rows, cursor)),
            Err(e) => return Err(e.into()),
        };
        for row in found {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            rows.push(row);
            if rows.len() >= want {
                let next_cursor = rows.last().map(|r| search_cursor(r.registered_at, &r.id));
                return Ok(FoundPage { rows, next_cursor, partial: false });
            }
        }
        if exhausted {
            return Ok(FoundPage { rows, next_cursor: None, partial: false });
        }
        cursor = Some(last);
        if std::time::Instant::now() >= deadline {
            return Ok(partial(rows, cursor));
        }
    }
}

/// Parents a page's nesting children point at but the page doesn't carry.
fn missing_parent_ids(sessions: &[SessionListItem]) -> Vec<String> {
    let here: HashSet<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
    let mut out: Vec<String> = Vec::new();
    for s in sessions {
        let relation = s.metadata.get("relation").and_then(serde_json::Value::as_str);
        let Some(pid) = s.parent_id.as_deref() else {
            continue;
        };
        if matches!(relation, Some("fork" | "followup")) || here.contains(pid) {
            continue;
        }
        if !out.iter().any(|p| p == pid) {
            out.push(pid.to_owned());
        }
    }
    out
}

#[derive(sqlx::FromRow)]
struct ParentStubRow {
    id: String,
    session_name: Option<String>,
    working_dir: String,
    status: String,
    registered_at: DateTime<Utc>,
    last_heartbeat: DateTime<Utc>,
}

async fn fetch_parent_stubs(
    pool: &sqlx::PgPool,
    ids: &[String],
    uid: Option<uuid::Uuid>,
) -> Result<Vec<SessionParentStub>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows: Vec<ParentStubRow> = sqlx::query_as(
        "SELECT s.id, s.session_name, s.working_dir, s.status, s.registered_at, s.last_heartbeat \
         FROM sessions s LEFT JOIN machines m ON m.id = s.machine_uuid \
         WHERE s.id = ANY($1) AND ($2::uuid IS NULL OR m.user_id = $2)",
    )
    .bind(ids)
    .bind(uid)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| SessionParentStub {
            status: resolve_status_liveness(&r.status, r.registered_at, r.last_heartbeat).0,
            id: r.id,
            name: r.session_name,
            working_dir: r.working_dir,
        })
        .collect())
}

/// Sessions with no matching event keep both fields `None`; clients read that
/// as an id/name/dir-only match and open the drawer normally.
fn attach_transcript_hits(
    sessions: &mut [SessionListItem],
    rows: Vec<(String, String, i64)>,
    terms: &[String],
) {
    let mut by_session: std::collections::HashMap<String, (String, i64)> =
        rows.into_iter().map(|(id, text, seq)| (id, (make_snippet(&text, terms), seq))).collect();
    for s in sessions {
        if let Some((snippet, seq)) = by_session.remove(&s.id) {
            s.match_snippet = Some(snippet);
            s.match_seq = Some(seq);
        }
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct FieldValuesParams {
    pub field: String,
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub context: Option<String>,
}

const FIELD_VALUES_LIMIT: i64 = 50;

/// DISTINCT-values query for a dynamic autocomplete field. Trailing binds are
/// `$N+1` = owner uuid, `$N+2` = the `q` ILIKE pattern (after the `$1..$N` from
/// `context`). Empty/whitespace `context` parses to `TRUE` (context-free).
fn field_values_sql(field: &str, context: &str) -> Option<(String, Vec<SqlParam>)> {
    let (joins, value_expr, guard): (&str, &str, &str) = match field {
        "machine" => (
            "",
            "COALESCE(m.display_name, m.name)",
            "m.kind = 'persistent' AND m.deleted_at IS NULL",
        ),
        "account" => (
            "JOIN session_tokens st ON st.session_id = s.id AND st.revoked_at IS NULL \
             JOIN account_providers ap ON ap.id = st.account_id \
             JOIN accounts a ON a.id = ap.account_id",
            "a.name",
            "TRUE",
        ),
        "tag" => (
            "JOIN session_labels sl ON sl.session_id = s.id \
             JOIN labels l ON l.id = sl.label_id",
            "l.name",
            "TRUE",
        ),
        "model" => (
            "",
            "COALESCE(s.model, s.metadata->>'model')",
            "COALESCE(s.model, s.metadata->>'model', '') <> ''",
        ),
        _ => return None,
    };

    let mut ctx_params: Vec<SqlParam> = Vec::new();
    let context_sql = compile_node(&cctui_query::parse(context.trim()), &mut ctx_params);
    let n = ctx_params.len();
    let (ui, qi) = (n + 1, n + 2);
    let sql = format!(
        "SELECT DISTINCT {value_expr} AS v \
         FROM sessions s \
         LEFT JOIN machines m ON m.id = s.machine_uuid \
         {joins} \
         WHERE ({context_sql}) AND ({guard}) \
         AND (${ui}::uuid IS NULL OR m.user_id = ${ui}) \
         AND (${qi}::text IS NULL OR {value_expr} ILIKE ${qi}) \
         ORDER BY v LIMIT {FIELD_VALUES_LIMIT}"
    );
    Some((sql, ctx_params))
}

/// `GET /sessions/search/values?field=…&q=…`: autocomplete suggestions
/// for a search field. Static enums come from the query registry; dynamic
/// fields (machine/account/tag/model) are distinct values from the caller's own
/// sessions (admin sees all). Machine suggestions exclude ephemeral/dispatch
/// worker machines and soft-deleted rows — searching them by name still works,
/// they're just not suggested. Unknown fields → empty list, never an error.
pub async fn search_field_values(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Query(params): Query<FieldValuesParams>,
) -> Result<Json<Vec<String>>, AppError> {
    check_query_len(params.context.as_deref().unwrap_or(""))?;
    check_query_len(params.q.as_deref().unwrap_or(""))?;
    let uid = ctx.owner_filter();
    let Some(def) = cctui_query::resolve(&params.field) else {
        return Ok(Json(vec![]));
    };
    if !def.enum_values.is_empty() {
        let prefix = params.q.unwrap_or_default().to_lowercase();
        return Ok(Json(
            def.enum_values
                .iter()
                .filter(|v| v.to_lowercase().starts_with(&prefix))
                .map(ToString::to_string)
                .collect(),
        ));
    }
    let like = params.q.as_deref().filter(|s| !s.is_empty()).map(ilike_contains);

    let Some((sql, ctx_params)) =
        field_values_sql(def.name, params.context.as_deref().unwrap_or(""))
    else {
        return Ok(Json(vec![]));
    };

    let mut query = sqlx::query_as::<_, (String,)>(sqlx::AssertSqlSafe(sql));
    for p in &ctx_params {
        query = match p {
            SqlParam::Text(s) => query.bind(s.clone()),
            SqlParam::Bool(b) => query.bind(*b),
        };
    }
    let rows: Vec<(String,)> = query.bind(uid).bind(&like).fetch_all(&state.pool).await?;
    Ok(Json(rows.into_iter().map(|(v,)| v).collect()))
}

type EndRow = (
    Option<String>,
    Option<String>,
    Option<DateTime<Utc>>,
    Option<serde_json::Value>,
    Option<String>,
    bool,
    Option<String>,
);

#[allow(clippy::too_many_lines)]
pub async fn get_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<SessionListItem>, AppError> {
    // Live session — serve straight from the registry.
    {
        let registry = state.registry.read().await;
        if let Some(handle) = registry.get(&session_id) {
            let permission_mode: Option<String> = sqlx::query_as::<_, (Option<String>,)>(
                "SELECT permission_mode FROM sessions WHERE id = $1",
            )
            .bind(&session_id)
            .fetch_optional(&state.pool)
            .await
            .ok()
            .flatten()
            .and_then(|(m,)| m);
            let mut item = SessionListItem {
                id: handle.session.id.clone(),
                parent_id: handle.session.parent_id.clone(),
                machine_id: handle.session.machine_id.clone(),
                working_dir: handle.session.working_dir.clone(),
                status: derive_status(handle.session.registered_at, handle.session.last_heartbeat),
                liveness: derive_liveness(handle.session.last_heartbeat),
                attention: None,
                bucket: Bucket::Working,
                token_usage: handle.token_usage.clone(),
                origin: cctui_proto::api::SessionOrigin::from_metadata(&handle.session.metadata),
                metadata: handle.session.metadata.clone(),
                adapter_id: handle.session.adapter_id.clone(),
                machine_name: None,
                machine_hue: None,
                machine_kind: None,
                last_message_text: None,
                last_message_at: None,
                registered_at: Some(handle.session.registered_at),
                name: None,
                model: None,
                effort: None,
                permission_mode,
                auto_approve: state
                    .permission_store
                    .read()
                    .await
                    .is_auto_approve(&handle.session.id),
                match_snippet: None,
                match_seq: None,
                last_activity_at: None,
                cache_cold: false,
                estimated_burst_tokens: None,
                hibernated: false,
                pinned: false,
                labels: Vec::new(),
                room_id: None,
                room_name: None,
                last_heartbeat: Some(handle.session.last_heartbeat),
                account_name: None,
                unread_count: 0,
                activity_detail: None,
                last_tool_at: None,
                last_tool_name: None,
                tool_use_count: 0,
                todos: Vec::new(),
                user_actions: None,
                context_counts: None,
                has_token_credentials: false,
                account_traffic_observed: false,
                pr_links: Vec::new(),
                end_reason: None,
                end_detail: None,
                ended_at: None,
                auto_archive_at: None,
                archived_by: None,
                keepalive: None,
                last_keepalive_at: None,
                launch_at: None,
                launch_error: None,
                next_session_id: None,
            };
            item.user_actions =
                fetch_user_actions(&state, std::slice::from_ref(&item.id)).await?.remove(&item.id);
            item.next_session_id = crate::followup::next_of(&state.pool, &item.id).await?;
            return Ok(Json(item));
        }
    }

    // Not live — fall back to the DB so archived/ended sessions still open
    // (read-only). A true 404 now means the session was actually deleted, not
    // just archived (item 6 — kills the spurious "not found or
    // archived" toast on refresh).
    let row: Option<DbSession> =
        crate::store::sessions::fetch_by_id(&state.pool, &session_id).await?;

    let row = row.ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "session not found"))?;

    let (status, liveness) =
        resolve_status_liveness(&row.status, row.registered_at, row.last_heartbeat);
    let mut item = SessionListItem {
        id: row.id.clone(),
        parent_id: row.parent_id,
        machine_id: row.machine_id,
        working_dir: row.working_dir,
        status,
        liveness,
        attention: None,
        bucket: Bucket::Working,
        token_usage: cctui_proto::models::TokenUsage::default(),
        origin: cctui_proto::api::SessionOrigin::from_metadata(&row.metadata),
        metadata: row.metadata,
        adapter_id: row.adapter_id.map(cctui_proto::adapter::AdapterId::new),
        machine_name: row.resolved_machine_name,
        machine_hue: row.resolved_machine_hue,
        machine_kind: row.resolved_machine_kind,
        last_message_text: None,
        last_message_at: None,
        registered_at: Some(row.registered_at),
        name: None,
        model: None,
        effort: None,
        permission_mode: None,
        auto_approve: state.permission_store.read().await.is_auto_approve(&row.id),
        match_snippet: None,
        match_seq: None,
        last_activity_at: None,
        cache_cold: false,
        estimated_burst_tokens: None,
        hibernated: false,
        pinned: false,
        labels: Vec::new(),
        room_id: row.room_id.map(|id| id.to_string()),
        room_name: row.room_name,
        last_heartbeat: Some(row.last_heartbeat),
        account_name: None,
        unread_count: 0,
        activity_detail: None,
        last_tool_at: None,
        last_tool_name: None,
        tool_use_count: 0,
        todos: Vec::new(),
        user_actions: None,
        context_counts: None,
        has_token_credentials: false,
        account_traffic_observed: false,
        pr_links: Vec::new(),
        end_reason: None,
        end_detail: None,
        ended_at: None,
        auto_archive_at: None,
        archived_by: None,
        keepalive: None,
        last_keepalive_at: None,
        launch_at: None,
        launch_error: None,
        next_session_id: None,
    };
    let end: Option<EndRow> = sqlx::query_as(
        "SELECT end_reason, end_detail, ended_at, todos, permission_mode, pinned, archived_by \
         FROM sessions WHERE id = $1",
    )
    .bind(&item.id)
    .fetch_optional(&state.pool)
    .await?;
    if let Some((end_reason, end_detail, ended_at, todos, permission_mode, pinned, archived_by)) =
        end
    {
        item.end_reason = end_reason.as_deref().map(SessionEndReason::parse);
        item.end_detail = end_detail;
        item.ended_at = ended_at;
        item.todos = todos.and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
        item.permission_mode = permission_mode;
        item.pinned = pinned;
        item.archived_by = archived_by.as_deref().and_then(RemoveInitiator::parse);
        item.auto_archive_at = crate::auto_archive::stale_archive_due(
            item.status,
            pinned,
            item.last_heartbeat,
            state.config.archive_after_secs,
        );
    }
    item.user_actions =
        fetch_user_actions(&state, std::slice::from_ref(&item.id)).await?.remove(&item.id);
    item.next_session_id = crate::followup::next_of(&state.pool, &item.id).await?;
    Ok(Json(item))
}

#[derive(Debug, Default, serde::Deserialize)]
pub struct ConversationQuery {
    /// Newest `limit` events only; absent = full transcript.
    pub limit: Option<i64>,
    /// Exclusive upper `seq` bound for paging back.
    pub before: Option<i64>,
    /// Exclusive lower `seq` bound for delta catch-up.
    pub after: Option<i64>,
    /// Which end of the range `limit` takes: `desc` (default) keeps the newest
    /// events, `asc` the oldest. The response is always oldest-first.
    #[serde(default)]
    pub order: ConversationOrder,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConversationOrder {
    #[default]
    Desc,
    Asc,
}

const fn conversation_sql(order: ConversationOrder) -> &'static str {
    match order {
        ConversationOrder::Desc => {
            "SELECT id, event_type, payload, created_at, turn_id FROM stream_events \
             WHERE session_id = $1 AND ($2::bigint IS NULL OR id < $2) \
               AND ($4::bigint IS NULL OR id > $4) \
             ORDER BY id DESC LIMIT $3"
        }
        ConversationOrder::Asc => {
            "SELECT id, event_type, payload, created_at, turn_id FROM stream_events \
             WHERE session_id = $1 AND ($2::bigint IS NULL OR id < $2) \
               AND ($4::bigint IS NULL OR id > $4) \
             ORDER BY id ASC LIMIT $3"
        }
    }
}

/// `(id, event_type, payload, created_at, turn_id)` — the column list of
/// [`conversation_sql`], in order.
type ConversationRow = (i64, String, serde_json::Value, DateTime<Utc>, Option<uuid::Uuid>);

/// `(id, client payload, created_at, turn_id)`: a stored row the client can
/// render, in the query's own order (newest-first for `Desc`).
pub type RenderableRow = (i64, serde_json::Value, DateTime<Utc>, Option<uuid::Uuid>);

/// Reads rows until `limit` of them survive [`crate::normalize::for_client`],
/// or the table is exhausted in the paging direction. Some stored rows carry
/// nothing renderable (turn bookkeeping, unknown roles); counting the limit
/// against raw rows let them shorten a page, which the client reads as "end of
/// transcript". Callers may rely on it: a page shorter than `limit` has no
/// more rows beyond it.
///
/// Orders by `id` (the BIGSERIAL insert sequence), not `created_at`: `id` is
/// the causal `seq` and a strict total order, so a late-flushed
/// `AskUserQuestion` card+preamble keep their insert position even when their
/// `created_at` ties or lands after the user's answer.
pub async fn renderable_rows(
    pool: &sqlx::PgPool,
    session_id: &str,
    adapter_id: &str,
    params: &ConversationQuery,
) -> Result<Vec<RenderableRow>, sqlx::Error> {
    let limit = params.limit.map(|l| l.clamp(1, 10_000));
    let mut before = params.before;
    let mut after = params.after;
    let mut out: Vec<RenderableRow> = Vec::new();
    loop {
        let want = limit.map(|l| l - i64::try_from(out.len()).unwrap_or(i64::MAX));
        let rows: Vec<ConversationRow> = sqlx::query_as(conversation_sql(params.order))
            .bind(session_id)
            .bind(before)
            .bind(want)
            .bind(after)
            .fetch_all(pool)
            .await?;
        let fetched = i64::try_from(rows.len()).unwrap_or(i64::MAX);
        let last_id = rows.last().map(|r| r.0);
        out.extend(rows.into_iter().filter_map(
            |(id, event_type, payload, created_at, turn_id)| {
                crate::normalize::for_client(adapter_id, &event_type, payload)
                    .map(|v| (id, v, created_at, turn_id))
            },
        ));
        let Some(limit) = limit else { return Ok(out) };
        let exhausted = want.is_some_and(|w| fetched < w);
        if exhausted || out.len() >= usize::try_from(limit).unwrap_or(usize::MAX) {
            return Ok(out);
        }
        match params.order {
            ConversationOrder::Desc => before = last_id,
            ConversationOrder::Asc => after = last_id,
        }
    }
}

/// `(message_id, model, input, output, cache_read, cache_creation,
/// gateway_rewrote_body, created_at)` from `session_token_usage`.
type UsageRow = (String, Option<String>, i64, i64, i64, i64, bool, DateTime<Utc>);

/// Turns before a page's first turn read to rebuild the agent streams its
/// cache-bust verdicts are judged against.
const USAGE_LOOKBACK: i64 = 40;

/// Usage rows a conversation page needs: every turn in the time span of the
/// page's `message_ids`, plus the [`USAGE_LOOKBACK`] turns before it.
async fn page_usage_rows(
    pool: &sqlx::PgPool,
    session_id: &str,
    message_ids: &[String],
) -> Result<Vec<UsageRow>, sqlx::Error> {
    if message_ids.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_as(
        "WITH span AS ( \
             SELECT min(created_at) AS lo, max(created_at) AS hi FROM session_token_usage \
             WHERE session_id = $1 AND message_id = ANY($2)) \
         SELECT u.message_id, u.model, u.input_tokens, u.output_tokens, u.cache_read_tokens, \
                u.cache_creation_tokens, u.gateway_rewrote_body, u.created_at \
         FROM span JOIN session_token_usage u \
           ON u.session_id = $1 AND u.created_at BETWEEN span.lo AND span.hi \
         UNION ALL \
         (SELECT u.message_id, u.model, u.input_tokens, u.output_tokens, u.cache_read_tokens, \
                 u.cache_creation_tokens, u.gateway_rewrote_body, u.created_at \
          FROM span JOIN session_token_usage u \
            ON u.session_id = $1 AND u.created_at < span.lo \
          ORDER BY u.created_at DESC LIMIT $3)",
    )
    .bind(session_id)
    .bind(message_ids)
    .bind(USAGE_LOOKBACK)
    .fetch_all(pool)
    .await
}

/// Per-message token usage for a page's `message_ids`, each turn carrying the
/// cache bust it suffered (if any) against the previous turn of its own agent
/// stream.
async fn message_usage(
    state: &AppState,
    session_id: &str,
    message_ids: &[String],
) -> Result<HashMap<String, cctui_proto::models::TokenUsage>, AppError> {
    let usage_rows = page_usage_rows(&state.pool, session_id, message_ids).await?;
    let wanted: HashSet<&str> = message_ids.iter().map(String::as_str).collect();
    let owned_id = session_id.to_owned();
    let catalog = session_catalogs(state, std::slice::from_ref(&owned_id)).await.remove(session_id);
    let turns: Vec<crate::cache_bust::Turn> = usage_rows
        .iter()
        .map(|(message_id, model, input, _, cache_read, cache_creation, rewrote, at)| {
            crate::cache_bust::Turn {
                message_id: message_id.clone(),
                model: model.clone(),
                input: *input,
                cache_read: *cache_read,
                cache_creation: *cache_creation,
                created_at: *at,
                gateway_rewrote_body: *rewrote,
            }
        })
        .collect();
    let mut busts = crate::cache_bust::compute(&turns, catalog.as_ref());
    Ok(usage_rows
        .into_iter()
        .filter(|row| wanted.contains(row.0.as_str()))
        .map(|(message_id, model, input, output, cache_read, cache_creation, _, _)| {
            let to_u64 = |v: i64| u64::try_from(v).unwrap_or(0);
            let cost = crate::cost::tallies_cost_usd(
                catalog.as_ref(),
                &[(model, crate::cost::TokenUsage { input, cached_input: cache_read, output })],
            );
            let cache_bust = busts.remove(&message_id).map(|b| cctui_proto::models::CacheBust {
                lost_tokens: b.lost_tokens,
                lost_usd: b.lost_usd,
                reason: b.reason.as_str().to_owned(),
            });
            (
                message_id,
                cctui_proto::models::TokenUsage {
                    tokens_in: to_u64(input),
                    tokens_out: to_u64(output),
                    cost_usd: cost,
                    cache_read_tokens: to_u64(cache_read),
                    cache_creation_tokens: to_u64(cache_creation),
                    cache_bust,
                },
            )
        })
        .collect())
}

pub async fn get_conversation(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(params): Query<ConversationQuery>,
    req_headers: axum::http::HeaderMap,
) -> Result<axum::response::Response, AppError> {
    let adapter: Option<String> =
        crate::store::sessions::adapter_id(&state.pool, &session_id).await?;

    let adapter_id = adapter.as_deref().unwrap_or("claude-code");
    let mut rows = renderable_rows(&state.pool, &session_id, adapter_id, &params).await?;
    if params.order == ConversationOrder::Desc {
        rows.reverse();
    }

    let message_ids: Vec<String> = rows
        .iter()
        .filter_map(|(_, v, _, _)| v.get("message_id").and_then(serde_json::Value::as_str))
        .map(str::to_owned)
        .collect();
    let usage_by_message = message_usage(&state, &session_id, &message_ids).await?;
    let scheduled_turns =
        crate::scheduled_messages::scheduled_turns(&state.pool, &session_id).await?;

    // Stamp each event with `ts` (unix millis, matching the live `AgentEvent`
    // shape) derived from `created_at`, so the client renders real timestamps
    // instead of "Invalid Date". Legacy payloads that already carry `ts` keep
    // their own value.
    let normalized: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(id, mut v, created_at, turn_id)| {
            if let Some(obj) = v.as_object_mut() {
                obj.entry("ts").or_insert_with(|| serde_json::json!(created_at.timestamp_millis()));
                obj.insert("seq".to_owned(), serde_json::json!(id));
                if let Some(turn_id) = turn_id {
                    obj.insert("turn_id".to_owned(), serde_json::json!(turn_id));
                    if let Some(at) = scheduled_turns.get(&turn_id) {
                        obj.insert(
                            "metadata".to_owned(),
                            serde_json::json!({ "scheduled_at": at.to_rfc3339() }),
                        );
                    }
                }
                if let Some(message_id) = obj.get("message_id").and_then(serde_json::Value::as_str)
                    && let Some(usage) = usage_by_message.get(message_id)
                {
                    obj.insert("usage".to_owned(), serde_json::json!(usage));
                }
            }
            v
        })
        .collect();
    Ok(crate::http_cache::json_with_etag(&req_headers, &normalized))
}

/// Newest renderable rows scanned for the final assistant message; one
/// message never spans more.
const RESULT_SCAN_LIMIT: i64 = 200;

/// A visible assistant line: claude-code rows carry a `message_id` and no
/// `role`, codex rows a `role` of `Assistant` and no `message_id`. Thinking
/// and markers carry a `kind`; user lines carry neither key.
fn is_final_text_candidate(v: &serde_json::Value) -> bool {
    let str_at = |k: &str| v.get(k).and_then(serde_json::Value::as_str);
    str_at("type") == Some("text")
        && str_at("kind").is_none()
        && v.get("meta").and_then(serde_json::Value::as_bool) != Some(true)
        && (str_at("message_id").is_some() || str_at("role") == Some("Assistant"))
}

/// `(message_id, text)` of the last assistant message in `newest_first`:
/// its trailing visible text rows joined in seq order. Rows of the same
/// message that are not visible text (thinking) are skipped; any other row
/// ends the message.
fn final_assistant_text(newest_first: &[RenderableRow]) -> (Option<String>, Option<String>) {
    fn message_id_of(v: &serde_json::Value) -> Option<&str> {
        v.get("message_id").and_then(serde_json::Value::as_str)
    }
    let mut rows =
        newest_first.iter().map(|(_, v, _, _)| v).skip_while(|v| !is_final_text_candidate(v));
    let Some(anchor) = rows.next() else {
        return (None, None);
    };
    let message_id = message_id_of(anchor).map(str::to_owned);
    let mut parts: Vec<&str> = vec![anchor["content"].as_str().unwrap_or_default()];
    for v in rows {
        let same_message = message_id.is_some() && message_id_of(v) == message_id.as_deref();
        if is_final_text_candidate(v) && message_id_of(v) == message_id.as_deref() {
            parts.push(v["content"].as_str().unwrap_or_default());
        } else if !same_message {
            break;
        }
    }
    parts.reverse();
    (message_id, Some(parts.join("\n\n")))
}

#[derive(sqlx::FromRow)]
struct ResultRow {
    adapter_id: Option<String>,
    end_reason: Option<String>,
    ended_at: Option<DateTime<Utc>>,
    status: String,
    tempo: Option<String>,
    agent_state: Option<String>,
    activity: Option<String>,
    soft_limit_reason: Option<String>,
}

impl ResultRow {
    /// Ended, archived, or idle after a finished turn: the bucket the list
    /// files as `Done`, never while the adapter still reports the turn busy.
    fn done(&self) -> bool {
        if self.ended_at.is_some() || self.status == SessionRowStatus::Archived.as_str() {
            return true;
        }
        if matches!(self.adapter_id.as_deref(), Some("codex" | "opencode")) {
            return self.idle_after_turn();
        }
        let busy = matches!(self.tempo.as_deref(), Some("active" | "blocked"))
            || matches!(self.agent_state.as_deref(), Some("working" | "running"));
        let bucket = bucket_from_signals(
            self.tempo.as_deref(),
            self.agent_state.as_deref(),
            self.activity.as_deref(),
            self.soft_limit_reason.as_deref(),
            &[],
            &HashMap::new(),
        );
        bucket == Bucket::Done && !busy
    }

    /// codex keeps `tempo` at its last busy value after the turn goes idle,
    /// so only `agent_state` is current for these adapters.
    fn idle_after_turn(&self) -> bool {
        self.soft_limit_reason.is_none()
            && matches!(self.agent_state.as_deref(), Some("idle" | "done"))
    }
}

pub async fn get_result(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<SessionResult>, AppError> {
    let row: Option<ResultRow> = sqlx::query_as(
        "SELECT adapter_id, end_reason, ended_at, status, tempo, agent_state, activity, \
                soft_limit_reason \
         FROM sessions WHERE id = $1",
    )
    .bind(&session_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some(row) = row else {
        return queued_dispatch_result(&state.pool, &session_id)
            .await?
            .map(Json)
            .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "session not found"));
    };
    let params = ConversationQuery { limit: Some(RESULT_SCAN_LIMIT), ..Default::default() };
    let rows = renderable_rows(
        &state.pool,
        &session_id,
        row.adapter_id.as_deref().unwrap_or("claude-code"),
        &params,
    )
    .await?;
    let (message_id, final_assistant_text) = final_assistant_text(&rows);
    let token_usage =
        message_usage(&state, &session_id, message_id.as_slice()).await?.into_values().next();
    Ok(Json(SessionResult {
        done: row.done(),
        end_reason: row.end_reason,
        final_assistant_text,
        message_id,
        token_usage,
        queued: false,
        queue_position: None,
    }))
}

/// A dispatch still held in its dispatcher's queue has no session row yet.
///
/// Its position counts the same dispatcher and profile's queued records.
async fn queued_dispatch_result(
    pool: &sqlx::PgPool,
    session_id: &str,
) -> Result<Option<SessionResult>, sqlx::Error> {
    let position: Option<i64> = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM dispatch_records q \
                 WHERE q.queued AND q.outcome = 'forwarded' \
                   AND q.dispatcher_name = r.dispatcher_name \
                   AND q.profile IS NOT DISTINCT FROM r.profile \
                   AND (q.created_at, q.session_id) <= (r.created_at, r.session_id)) \
         FROM dispatch_records r \
         WHERE r.session_id = $1 AND r.queued AND r.outcome = 'forwarded'",
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await?;
    Ok(position.map(|p| SessionResult {
        done: false,
        end_reason: None,
        final_assistant_text: None,
        message_id: None,
        token_usage: None,
        queued: true,
        queue_position: u32::try_from(p).ok(),
    }))
}

#[derive(Debug, Default, Deserialize)]
pub struct ConversationSearchParams {
    #[serde(default)]
    pub q: String,
    pub limit: Option<i64>,
}

/// Hits past this are dropped and the response reports `truncated`.
const CONV_SEARCH_MAX_HITS: usize = 500;

/// Rows the indexed scan reads before stopping. Higher than the hit cap
/// because a scanned row may normalise to nothing renderable or be dropped by
/// a `role`/`tool`/`pinned` clause, none of which SQL can decide.
const CONV_SEARCH_SCAN_CAP: i64 = 20_000;

/// The in-conversation bar composes its clauses as one conjunction, so the AST
/// is flattened to `(every term) AND (every clause)`; `OR` nesting a user types
/// by hand collapses into that same conjunction.
///
/// `-role:x` / `role!:x` parse as a `Not` around the leaf (the parser never
/// emits `FilterOp::Ne`), so the wrapper is what carries negation down here.
fn flatten_conv_query(
    node: &cctui_query::Node,
    negated: bool,
    terms: &mut Vec<String>,
    out: &mut Vec<(bool, cctui_query::Filter)>,
) {
    use cctui_query::Node;
    match node {
        Node::Empty => {}
        Node::Text { value } => terms.push(value.clone()),
        Node::Filter { filter } => out.push((negated, filter.clone())),
        Node::Not { child } => flatten_conv_query(child, !negated, terms, out),
        Node::And { children } | Node::Or { children } => {
            for c in children {
                flatten_conv_query(c, negated, terms, out);
            }
        }
    }
}

#[derive(Debug, Default)]
struct ConvSearchFilters {
    roles: Vec<String>,
    not_roles: Vec<String>,
    tools: Vec<String>,
    not_tools: Vec<String>,
    pinned: Option<bool>,
    after: Option<DateTime<Utc>>,
    before: Option<DateTime<Utc>>,
}

/// A bare `YYYY-MM-DD` means that day's UTC midnight; a full RFC 3339 stamp is
/// taken as given.
fn parse_conv_bound(v: &str) -> Option<DateTime<Utc>> {
    let v = v.trim();
    if let Ok(dt) = DateTime::parse_from_rfc3339(v) {
        return Some(dt.with_timezone(&Utc));
    }
    let day = chrono::NaiveDate::parse_from_str(v, "%Y-%m-%d").ok()?;
    Some(DateTime::from_naive_utc_and_offset(day.and_hms_opt(0, 0, 0)?, Utc))
}

/// `tool:mcp__github__*` matches by prefix; anything else is exact, both
/// case-insensitively.
fn tool_matches(pattern: &str, tool: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix('*') {
        return tool.to_lowercase().starts_with(&prefix.to_lowercase());
    }
    tool.eq_ignore_ascii_case(pattern)
}

fn conv_filters(filters: &[(bool, cctui_query::Filter)]) -> ConvSearchFilters {
    let mut out = ConvSearchFilters::default();
    for (negated, f) in filters {
        let negated = *negated;
        match f.field.as_str() {
            "role" => {
                let bucket = if negated { &mut out.not_roles } else { &mut out.roles };
                bucket.extend(f.values.iter().map(|v| v.to_lowercase()));
            }
            "tool" => {
                let bucket = if negated { &mut out.not_tools } else { &mut out.tools };
                bucket.extend(f.values.iter().cloned());
            }
            "pinned" => {
                let want = f.values.first().is_none_or(|v| v.eq_ignore_ascii_case("true"));
                out.pinned = Some(want != negated);
            }
            "after" => out.after = f.values.first().map(String::as_str).and_then(parse_conv_bound),
            "before" => {
                out.before = f.values.first().map(String::as_str).and_then(parse_conv_bound);
            }
            _ => {}
        }
    }
    out
}

impl ConvSearchFilters {
    fn accepts(&self, role: &str, tool: Option<&str>, seq: i64, pinned: &HashSet<i64>) -> bool {
        if role.is_empty() {
            return false;
        }
        if !self.roles.is_empty() && !self.roles.iter().any(|r| r.as_str() == role) {
            return false;
        }
        if self.not_roles.iter().any(|r| r.as_str() == role) {
            return false;
        }
        if !self.tools.is_empty()
            && !tool.is_some_and(|t| self.tools.iter().any(|p| tool_matches(p, t)))
        {
            return false;
        }
        if self.not_tools.iter().any(|p| tool.is_some_and(|t| tool_matches(p, t))) {
            return false;
        }
        if let Some(want) = self.pinned
            && pinned.contains(&seq) != want
        {
            return false;
        }
        true
    }
}

/// `(id, event_type, payload, created_at)`.
type ConvSearchRow = (i64, String, serde_json::Value, DateTime<Utc>);

/// `$1` the session id, `$2…$n+1` the folded ILIKE patterns — all of them
/// required of one and the same event, each in any of its chunks — then
/// `after`, `before` and the scan cap.
fn conv_search_sql(n_patterns: usize) -> String {
    let text_where = if n_patterns == 0 {
        "TRUE".to_string()
    } else {
        let sets = (2..=n_patterns + 1)
            .map(|i| {
                format!(
                    "SELECT c.event_id FROM search_chunks c \
                     WHERE c.session_id = $1 AND c.body ILIKE ${i}"
                )
            })
            .collect::<Vec<_>>()
            .join(" INTERSECT ");
        format!("e.id IN ({sets})")
    };
    let (a, b, l) = (n_patterns + 2, n_patterns + 3, n_patterns + 4);
    format!(
        "SELECT e.id, e.event_type, e.payload, e.created_at \
         FROM stream_events e \
         WHERE e.session_id = $1 AND ({text_where}) \
           AND (${a}::timestamptz IS NULL OR e.created_at >= ${a}) \
           AND (${b}::timestamptz IS NULL OR e.created_at < ${b}) \
         ORDER BY e.id ASC LIMIT ${l}"
    )
}

/// The candidate rows of a conversation search, under
/// [`SEARCH_STATEMENT_TIMEOUT`]; `None` when it ran out of time.
async fn conv_search_rows(
    pool: &sqlx::PgPool,
    session_id: &str,
    terms: &[String],
    filters: &ConvSearchFilters,
) -> Result<Option<Vec<ConvSearchRow>>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query(SEARCH_STATEMENT_TIMEOUT).execute(&mut *tx).await?;
    let sql = conv_search_sql(terms.len());
    let mut query = sqlx::query_as::<_, ConvSearchRow>(sqlx::AssertSqlSafe(sql)).bind(session_id);
    for t in terms {
        query = query.bind(ilike_contains(t));
    }
    match query
        .bind(filters.after)
        .bind(filters.before)
        .bind(CONV_SEARCH_SCAN_CAP)
        .fetch_all(&mut *tx)
        .await
    {
        Ok(rows) => Ok(Some(rows)),
        Err(e) if is_statement_timeout(&e) => Ok(None),
        Err(e) => Err(e),
    }
}

/// `GET /sessions/{id}/search?q=…&limit=…`: the hit list for a find-in-
/// conversation bar, oldest → newest. Every free-text term, folded like the
/// rendered transcript, must match the same event's search chunks;
/// `role`/`type`/`tool`/`pinned` are decided on the normalised client payload
/// so the roles match the webui's `MsgCategory`.
#[allow(clippy::too_many_lines)]
pub async fn search_conversation(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(session_id): Path<String>,
    Query(params): Query<ConversationSearchParams>,
) -> Result<Json<cctui_proto::api::ConversationSearchResponse>, AppError> {
    check_query_len(&params.q)?;
    let root = cctui_query::parse(params.q.trim());
    let max_hits = params
        .limit
        .map_or(CONV_SEARCH_MAX_HITS, |l| usize::try_from(l.max(1)).unwrap_or(CONV_SEARCH_MAX_HITS))
        .min(CONV_SEARCH_MAX_HITS);
    if root.is_empty() {
        return Ok(Json(cctui_proto::api::ConversationSearchResponse {
            hits: Vec::new(),
            total: 0,
            truncated: false,
            tools: Vec::new(),
        }));
    }

    let mut terms = Vec::new();
    let mut raw_filters = Vec::new();
    flatten_conv_query(&root, false, &mut terms, &mut raw_filters);
    let filters = conv_filters(&raw_filters);
    let terms = fold_terms(&terms);

    let Some(rows) = conv_search_rows(&state.pool, &session_id, &terms, &filters).await? else {
        return Err(AppError::new(
            StatusCode::GATEWAY_TIMEOUT,
            "search timed out: add a word, quote a phrase or narrow it with after:/before:",
        ));
    };
    let scanned = i64::try_from(rows.len()).unwrap_or(i64::MAX);

    let pinned: HashSet<i64> = if filters.pinned.is_some() {
        sqlx::query_scalar::<_, i64>(
            "SELECT seq FROM session_message_pins WHERE user_id = $1 AND session_id = $2",
        )
        .bind(ctx.user_id)
        .bind(&session_id)
        .fetch_all(&state.pool)
        .await?
        .into_iter()
        .collect()
    } else {
        HashSet::new()
    };

    let adapter = crate::store::sessions::adapter_id(&state.pool, &session_id).await?;
    let adapter_id = adapter.as_deref().unwrap_or("claude-code");

    let mut hits: Vec<cctui_proto::api::ConversationHit> = Vec::new();
    let mut truncated = scanned >= CONV_SEARCH_SCAN_CAP;
    let mut tools: Vec<String> = Vec::new();
    for (id, event_type, payload, created_at) in rows {
        let Some(client) = crate::normalize::for_client(adapter_id, &event_type, payload) else {
            continue;
        };
        let (role, tool) = crate::normalize::client_category(&client);
        if !filters.accepts(role, tool.as_deref(), id, &pinned) {
            continue;
        }
        if let Some(t) = &tool
            && !tools.contains(t)
        {
            tools.push(t.clone());
        }
        if hits.len() >= max_hits {
            truncated = true;
            break;
        }
        hits.push(cctui_proto::api::ConversationHit {
            seq: id,
            ts: created_at.timestamp_millis(),
            role: role.to_owned(),
            tool,
            snippet: make_snippet(&crate::search_index::text::client_text(&client), &terms),
        });
    }
    let total = u32::try_from(hits.len()).unwrap_or(u32::MAX);
    Ok(Json(cctui_proto::api::ConversationSearchResponse { hits, total, truncated, tools }))
}

pub async fn send_message(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(session_id): Path<String>,
    Json(req): Json<MessageRequest>,
) -> Result<(StatusCode, Json<cctui_proto::api::SpawnResponse>), AppError> {
    if let Some(raw) = req.deliver_at.as_deref() {
        return crate::routes::scheduled_messages::schedule(
            &state,
            &ctx,
            &session_id,
            &req.content,
            raw,
        )
        .await;
    }
    authorize_reply(&state, &ctx, &session_id).await?;
    // Carry re-minted gateway env so a reply-driven cold-resume revives the
    // worker with a fresh valid token rather than empty env.
    let env = crate::routes::gateway::resume_env_for_session(&state, &session_id).await;
    let command_id = uuid::Uuid::new_v4();
    crate::state::track_command(
        &state.pending_commands,
        command_id,
        Some(session_id.clone()),
        None,
    );
    let dispatch = crate::bus::dispatch(
        &state,
        &session_id,
        cctui_proto::adapter::AdapterCommand::Reply {
            local_id: session_id.clone(),
            recover_auth: crate::auto_resume::should_recover_gateway_auth(&state, &session_id)
                .await,
            text: req.content,
            ask_picks: None,
            env,
            command_id: Some(command_id),
            turn_id: req.turn_id,
        },
    )
    .await;
    if let Err(err) = dispatch {
        use crate::bus::BusError;
        let status = match err {
            BusError::NotFound => StatusCode::NOT_FOUND,
            _ => StatusCode::SERVICE_UNAVAILABLE,
        };
        tracing::warn!(%session_id, %err, "message dispatch failed");
        return Err(AppError::new(status, err.to_string()));
    }
    Ok((
        StatusCode::ACCEPTED,
        Json(cctui_proto::api::SpawnResponse {
            command_id,
            status: "dispatched".into(),
            account: None,
            session_id: None,
        }),
    ))
}

/// A draft has no worker to wake.
pub fn reply_wakes_worker(status: &str, tempo: Option<&str>) -> bool {
    matches!(tempo, Some("hibernated" | "dead"))
        || matches!(
            SessionRowStatus::parse(status),
            Some(SessionRowStatus::Ended | SessionRowStatus::Failed | SessionRowStatus::Archived)
        )
}

/// A reply that wakes the worker is a launch and gated like a resume.
pub async fn authorize_reply(
    state: &AppState,
    ctx: &AuthContext,
    session_id: &str,
) -> Result<(), AppError> {
    let row: Option<(String, Option<String>)> =
        sqlx::query_as("SELECT status, tempo FROM sessions WHERE id = $1")
            .bind(session_id)
            .fetch_optional(&state.pool)
            .await?;
    match row {
        Some((status, tempo)) if reply_wakes_worker(&status, tempo.as_deref()) => {
            crate::routes::spawn::authorize_session_launch(state, ctx, session_id).await
        }
        _ => Ok(()),
    }
}

/// Store a user-chosen name and pin it against agent titles. `None` when the
/// session does not exist, else the name it had before.
pub async fn store_rename(
    pool: &sqlx::PgPool,
    session_id: &str,
    name: &str,
) -> Result<Option<Option<String>>, sqlx::Error> {
    sqlx::query_scalar(
        "WITH prior AS (SELECT session_name FROM sessions WHERE id = $1) \
         UPDATE sessions SET session_name = $2, \
            metadata = COALESCE(metadata, '{}'::jsonb) || jsonb_build_object('name_source', 'user') \
         WHERE id = $1 \
         RETURNING (SELECT session_name FROM prior)",
    )
    .bind(session_id)
    .bind(name)
    .fetch_optional(pool)
    .await
}

pub async fn rename_session(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(session_id): Path<String>,
    Json(req): Json<RenameRequest>,
) -> Result<StatusCode, AppError> {
    let name = req.name.trim();
    if name.is_empty() {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "name must not be empty"));
    }
    // Persist immediately so the UI reflects the rename without waiting for
    // the daemon round-trip. The daemon write-through (below) keeps the
    // on-disk state.json in sync so the next status poll doesn't revert it.
    let previous = store_rename(&state.pool, &session_id, name).await?;
    if let Some(previous) = previous
        && previous.as_deref() != Some(name)
    {
        events::record(
            &state,
            Event::new(kind::SESSION_RENAMED, Actor::User(ctx.user_id))
                .session(&session_id)
                .detail(serde_json::json!({ "from": previous, "to": name, "session_name": name })),
        );
    }
    // Best-effort propagation to the owning daemon's adapter.
    let _ = crate::bus::dispatch(
        &state,
        &session_id,
        cctui_proto::adapter::AdapterCommand::Rename {
            local_id: session_id.clone(),
            name: name.to_owned(),
        },
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /sessions/{id}/seen`: mark this session's messages seen for
/// the calling user by upserting the read high-water mark to `now()`. `GREATEST`
/// keeps it monotonic so a stale/out-of-order call can never rewind the cursor.
/// Owner-scoped by the `sess_write` route guard.
pub async fn mark_seen(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(session_id): Path<String>,
) -> Result<StatusCode, AppError> {
    sqlx::query(
        "INSERT INTO session_reads (user_id, session_id, last_seen_at) \
         VALUES ($1, $2, now()) \
         ON CONFLICT (user_id, session_id) \
         DO UPDATE SET last_seen_at = GREATEST(session_reads.last_seen_at, EXCLUDED.last_seen_at)",
    )
    .bind(ctx.user_id)
    .bind(&session_id)
    .execute(&state.pool)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn kill_session(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(session_id): Path<String>,
) -> Result<StatusCode, AppError> {
    // Recorded before dispatch so the recovery reaper cannot revive the same
    // failed turn while the native worker is stopping.
    if let Err(err) = events::record_now(
        &state,
        Event::new(kind::SESSION_KILLED, Actor::User(ctx.user_id)).session(&session_id),
    )
    .await
    {
        tracing::warn!(%err, session_id = %session_id, "lifecycle event insert failed");
    }

    // Best-effort: also dispatch to the daemon so the running worker is
    // actually killed via the `claude daemon` socket. The DB update
    // below remains source-of-truth.
    let _ = crate::bus::dispatch(
        &state,
        &session_id,
        cctui_proto::adapter::AdapterCommand::Kill { local_id: session_id.clone(), signal: None },
    )
    .await;
    // Kill drops the in-memory handle and marks the DB row inactive. The
    // session isn't archived — later activity can revive it.
    crate::store::sessions::set_inactive(&state.pool, &session_id, false).await?;
    {
        let mut registry = state.registry.write().await;
        registry.deregister(&session_id);
    }
    crate::guard_step::forget(&session_id);
    state.bus.deregister_session_stream(&session_id);
    tracing::info!(session_id = %session_id, "session killed");
    Ok(StatusCode::NO_CONTENT)
}

/// Stop the in-flight turn of `session_id` without tearing the session down,
/// for the interrupt route and the live voice loop alike.
///
/// Dispatches `Interrupt`, NOT a kill: the claude adapter injects an ESC
/// keystroke into the worker PTY, the codex adapter sends `turn/interrupt`
/// without terminating, so the DB row stays active and in the registry.
/// Mints a `command_id` the adapter echoes back in its `CommandResult`, so
/// the caller can learn whether the agent actually accepted the interrupt.
pub async fn dispatch_interrupt(
    state: &AppState,
    session_id: &str,
    user_id: uuid::Uuid,
) -> Result<uuid::Uuid, AppError> {
    let command_id = uuid::Uuid::new_v4();
    // Recorded before dispatch so the recovery reaper cannot revive the same
    // failed turn while the native worker is stopping.
    if let Err(err) = events::record_now(
        state,
        Event::new(kind::SESSION_INTERRUPTED, Actor::User(user_id))
            .session(session_id)
            .detail(serde_json::json!({ "command_id": command_id })),
    )
    .await
    {
        tracing::warn!(%err, session_id = %session_id, "lifecycle event insert failed");
    }
    crate::state::track_command(
        &state.pending_commands,
        command_id,
        Some(session_id.to_owned()),
        None,
    );
    let dispatch = crate::bus::dispatch(
        state,
        session_id,
        cctui_proto::adapter::AdapterCommand::Interrupt {
            local_id: session_id.to_owned(),
            command_id: Some(command_id),
        },
    )
    .await;
    if let Err(err) = dispatch {
        use crate::bus::BusError;
        state.pending_commands.remove(&command_id);
        tracing::warn!(%session_id, %err, "interrupt dispatch failed");
        let (status, message) = match err {
            BusError::NotFound => (StatusCode::NOT_FOUND, err.to_string()),
            BusError::Timeout => (
                StatusCode::SERVICE_UNAVAILABLE,
                "the daemon is not reading its connection; the interrupt was not delivered"
                    .to_owned(),
            ),
            _ => (StatusCode::SERVICE_UNAVAILABLE, err.to_string()),
        };
        return Err(AppError::new(status, message));
    }
    tracing::info!(session_id = %session_id, %command_id, "session interrupted");
    Ok(command_id)
}

pub async fn interrupt_session(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(session_id): Path<String>,
) -> Result<(StatusCode, Json<cctui_proto::api::SpawnResponse>), AppError> {
    let command_id = dispatch_interrupt(&state, &session_id, ctx.user_id).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(cctui_proto::api::SpawnResponse {
            command_id,
            status: "dispatched".into(),
            account: None,
            session_id: None,
        }),
    ))
}

/// Body of `POST /sessions/{id}/switch-account`. `account` is an
/// `accounts.name`, an identity UUID, or a credential UUID — a credential UUID
/// also names which per-family binding is rebound; the other forms use
/// `family` (`anthropic` | `openai` | `fireworks`), defaulting to the harness's
/// family.
#[derive(Deserialize)]
pub struct SwitchAccountRequest {
    pub account: String,
    #[serde(default)]
    pub family: Option<String>,
}

/// `POST /api/v1/sessions/{id}/switch-account` — rebind one of a session's
/// per-family credential bindings to another account of the same owner.
///
/// A worker carries one opaque gateway token per provider family
/// (`ANTHROPIC_AUTH_TOKEN` / `OPENAI_API_KEY`); the gateway resolves each per
/// request via `session_tokens`. Switching is a pure server-side rebind of the
/// token row in the target's family — same env tokens, no restart, and the
/// other family's binding survives (Codex spawns keep their account when the
/// Claude side moves). 409 when the session has no binding in the target's
/// family. Success clears the soft-limit block (`SoftLimitCleared`).
#[allow(clippy::too_many_lines)]
pub async fn switch_account(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(session_id): Path<String>,
    Json(req): Json<SwitchAccountRequest>,
) -> Result<StatusCode, AppError> {
    use crate::routes::gateway::Family;

    let meta: Option<(uuid::Uuid, Option<String>)> = sqlx::query_as(
        "SELECT a.user_id, s.adapter_id \
         FROM session_tokens t \
         JOIN account_providers a ON a.id = t.account_id \
         LEFT JOIN sessions s ON s.id = t.session_id \
         WHERE t.session_id = $1 AND t.revoked_at IS NULL \
         LIMIT 1",
    )
    .bind(&session_id)
    .fetch_optional(&state.pool)
    .await?;
    let Some((bound_account_owner, adapter_id)) = meta else {
        return Err(AppError::new(
            StatusCode::NOT_FOUND,
            "session has no active gateway account binding to switch",
        ));
    };
    // The user the session runs for, not the bound account's owner: on a shared
    // account the two differ, and resolving the target against the owner lets a
    // grantee move their session onto accounts of the sharer's that were never
    // shared.
    let runs_for = crate::routes::gateway::acting_user(&state, &session_id, None)
        .await
        .unwrap_or(bound_account_owner);
    // The machine owner of a session a spawn grantee runs moves it onto their
    // own accounts and takes the binding over, never onto the grantee's others.
    let takeover = (!ctx.is_admin() && ctx.user_id != runs_for).then_some(ctx.user_id);
    let acting_user = takeover.unwrap_or(runs_for);
    let default_family = match req.family.as_deref().map(str::trim) {
        None | Some("") => {
            let adapter = adapter_id.as_deref().unwrap_or("claude-code");
            Family::try_from_adapter(adapter).ok_or_else(|| {
                AppError::new(
                    StatusCode::BAD_REQUEST,
                    format!("adapter {adapter:?} has no provider family; name one"),
                )
            })?
        }
        Some(label) => match Family::from_label(label) {
            Some(f) => f,
            None => {
                return Err(AppError::new(
                    StatusCode::BAD_REQUEST,
                    "family must be `anthropic`, `openai` or `fireworks`",
                ));
            }
        },
    };

    // A UUID may be either level: a credential id — which names its
    // own family — or an identity id, resolved in `default_family` like a name.
    let target: Option<(uuid::Uuid, String)> =
        if let Ok(tid) = uuid::Uuid::parse_str(req.account.trim()) {
            let direct: Option<(uuid::Uuid, String)> = sqlx::query_as(
                "SELECT ap.id, ap.family \
                 FROM account_providers ap JOIN accounts a ON a.id = ap.account_id \
                 WHERE ap.id = $1 AND (a.user_id = $2 OR EXISTS ( \
                     SELECT 1 FROM resource_shares rs \
                      WHERE rs.resource_type = 'account' AND rs.resource_id = a.id \
                        AND rs.grantee_id = $2 AND rs.revoked_at IS NULL))",
            )
            .bind(tid)
            .bind(acting_user)
            .fetch_optional(&state.pool)
            .await?;
            if direct.is_some() {
                direct
            } else {
                sqlx::query_as(
                    "SELECT ap.id, ap.family \
                 FROM account_providers ap JOIN accounts a ON a.id = ap.account_id \
                 WHERE a.id = $1 AND ap.family = $3 AND (a.user_id = $2 OR EXISTS ( \
                     SELECT 1 FROM resource_shares rs \
                      WHERE rs.resource_type = 'account' AND rs.resource_id = a.id \
                        AND rs.grantee_id = $2 AND rs.revoked_at IS NULL)) \
                 LIMIT 1",
                )
                .bind(tid)
                .bind(acting_user)
                .bind(default_family.label())
                .fetch_optional(&state.pool)
                .await?
            }
        } else {
            sqlx::query_as(
                "SELECT ap.id, ap.family \
             FROM account_providers ap JOIN accounts a ON a.id = ap.account_id \
             WHERE a.name = $1 AND ap.family = $3 AND (a.user_id = $2 OR EXISTS ( \
                     SELECT 1 FROM resource_shares rs \
                      WHERE rs.resource_type = 'account' AND rs.resource_id = a.id \
                        AND rs.grantee_id = $2 AND rs.revoked_at IS NULL)) \
             ORDER BY (a.user_id = $2) DESC, a.created_at \
             LIMIT 1",
            )
            .bind(req.account.trim())
            .bind(acting_user)
            .bind(default_family.label())
            .fetch_optional(&state.pool)
            .await?
        };
    let Some((target_id, target_family)) = target else {
        return Err(AppError::new(
            StatusCode::NOT_FOUND,
            "no such account available to this session's user",
        ));
    };

    let current: Option<(uuid::Uuid,)> = sqlx::query_as(
        "SELECT ap.id \
         FROM session_tokens t JOIN account_providers ap ON ap.id = t.account_id \
         WHERE t.session_id = $1 AND t.revoked_at IS NULL AND ap.family = $2 \
         ORDER BY t.created_at DESC LIMIT 1",
    )
    .bind(&session_id)
    .bind(&target_family)
    .fetch_optional(&state.pool)
    .await?;
    let Some((current_account_id,)) = current else {
        return Err(AppError::new(
            StatusCode::CONFLICT,
            "session has no binding in the target's provider family; cross-provider switching is not supported",
        ));
    };

    if target_id == current_account_id {
        return Err(AppError::new(StatusCode::CONFLICT, "session is already on that account"));
    }

    // Repoint only this family's row: touching the other family's would hand
    // e.g. the worker's OPENAI_API_KEY an anthropic credential.
    let updated = sqlx::query(
        "UPDATE session_tokens SET account_id = $3, user_id = COALESCE($4, user_id) \
         WHERE session_id = $1 AND revoked_at IS NULL AND account_id = $2",
    )
    .bind(&session_id)
    .bind(current_account_id)
    .bind(target_id)
    .bind(takeover)
    .execute(&state.pool)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(AppError::new(StatusCode::NOT_FOUND, "no active token row to rebind"));
    }

    // The rebind reuses the SAME token string — clear any orphan-spam block on
    // its fingerprint so the fixed binding resolves immediately instead of
    // 401ing for the remainder of the (up to 300s) block window.
    crate::routes::gateway::clear_orphan_block_for_session(&state, &session_id).await;

    // Dismiss the per-chat soft-limit banner.
    crate::routes::gateway::clear_soft_limit_block(&state, &session_id).await;
    tracing::info!(
        session_id = %session_id,
        from = %current_account_id,
        to = %target_id,
        family = %target_family,
        "switched session account (soft-limit rebind)"
    );
    events::record(
        &state,
        Event::new(kind::SESSION_ACCOUNT_SWITCHED, Actor::User(ctx.user_id))
            .session(&session_id)
            .detail(serde_json::json!({
                "from_account_id": current_account_id,
                "to_account_id": target_id,
                "family": target_family,
            })),
    );
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/v1/sessions/{id}/bindings` — the session's active per-family
/// credential bindings, one entry per family it carries.
pub async fn session_bindings(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<Vec<SessionBinding>>, AppError> {
    let rows: Vec<(String, uuid::Uuid, uuid::Uuid, String)> = sqlx::query_as(
        "SELECT DISTINCT ON (ap.family) ap.family, ap.id, acc.id, acc.name \
         FROM session_tokens t \
         JOIN account_providers ap ON ap.id = t.account_id \
         JOIN accounts acc ON acc.id = ap.account_id \
         WHERE t.session_id = $1 AND t.revoked_at IS NULL \
         ORDER BY ap.family, t.created_at DESC",
    )
    .bind(&session_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|(family, credential_id, account_id, account_name)| SessionBinding {
                family,
                credential_id,
                account_id,
                account_name,
            })
            .collect(),
    ))
}

/// `POST /api/v1/sessions/{id}/resume` — explicitly revive an exited durable
/// conversation in place. The adapter reuses the original transcript identity;
/// this is not a fork.
pub async fn resume_session(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(session_id): Path<String>,
) -> Result<StatusCode, AppError> {
    crate::routes::spawn::authorize_session_launch(&state, &ctx, &session_id).await?;
    // Pass the working_dir so the daemon can resume even after archiving ran
    // `claude rm` (which deletes the on-disk job state.json but keeps the
    // conversation transcript) — the daemon falls back to local_id + this cwd.
    let working_dir: Option<String> =
        crate::store::sessions::working_dir(&state.pool, &session_id).await.ok().flatten();

    // Re-mint the gateway env for the session's bound OAuth account so the
    // revived worker keeps routing through the gateway instead of hitting the
    // default upstream with no credential and 401ing.
    let env = crate::routes::gateway::resume_env_for_session(&state, &session_id).await;

    crate::bus::dispatch(
        &state,
        &session_id,
        cctui_proto::adapter::AdapterCommand::Resume {
            local_id: session_id.clone(),
            working_dir,
            env,
        },
    )
    .await
    .map_err(|e| {
        tracing::warn!(%session_id, error = %e, "resume dispatch failed");
        AppError::new(StatusCode::SERVICE_UNAVAILABLE, format!("resume failed: {e}"))
    })?;
    crate::store::sessions::set_inactive(&state.pool, &session_id, true).await?;
    crate::routes::spawn::fill_spawner(&state.pool, &session_id, ctx.user_id).await;
    tracing::info!(session_id = %session_id, "resume dispatched");
    events::record(
        &state,
        Event::new(kind::SESSION_RESUMED, Actor::User(ctx.user_id))
            .session(&session_id)
            .detail(serde_json::json!({ "origin": "manual" })),
    );
    Ok(StatusCode::ACCEPTED)
}

/// `POST /api/v1/sessions/{id}/set-model` — change the model and/or reasoning
/// effort of a running session in place. Agent-asymmetric: the codex
/// adapter records the override and carries it on the next `turn/start` (a
/// stable per-turn override), then echoes the resolved model/effort
/// back as an `AdapterEvent::Status` (which updates the DB row + chip); the
/// claude-code adapter rejects it with a clear "fork to change model" error
/// (the webui pre-empts that by offering the fork affordance for claude
/// sessions).
///
/// Mints a `command_id` so the adapter echoes back an
/// `AdapterEvent::CommandResult` → `ServerEvent::CommandResult`; the webui
/// awaits it before confirming the change.
/// Returns the id in the response body, mirroring interrupt.
pub async fn set_model(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(session_id): Path<String>,
    Json(req): Json<cctui_proto::api::SetModelRequest>,
) -> Result<(StatusCode, Json<cctui_proto::api::SpawnResponse>), AppError> {
    let norm = |s: Option<String>| s.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());
    let model = norm(req.model);
    let effort = norm(req.effort);
    if model.is_none() && effort.is_none() {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "model or effort must be set"));
    }
    let command_id = uuid::Uuid::new_v4();
    crate::state::track_command(
        &state.pending_commands,
        command_id,
        Some(session_id.clone()),
        None,
    );
    let _ = crate::bus::dispatch(
        &state,
        &session_id,
        cctui_proto::adapter::AdapterCommand::SetModel {
            local_id: session_id.clone(),
            model: model.clone(),
            effort: effort.clone(),
            command_id: Some(command_id),
        },
    )
    .await;
    tracing::info!(session_id = %session_id, %command_id, ?model, ?effort, "set-model dispatched");
    events::record(
        &state,
        Event::new(kind::SESSION_MODEL_CHANGED, Actor::User(ctx.user_id))
            .session(&session_id)
            .detail(
                serde_json::json!({ "model": model, "effort": effort, "command_id": command_id }),
            ),
    );
    Ok((
        StatusCode::ACCEPTED,
        Json(cctui_proto::api::SpawnResponse {
            command_id,
            status: "dispatched".into(),
            account: None,
            session_id: None,
        }),
    ))
}

/// `POST /api/v1/sessions/{id}/fork` — fork an existing conversation into a
/// brand-new session, optionally changing model/effort at fork time.
///
/// Resolves the parent's `(adapter_id, machine_uuid, working_dir)` from the DB,
/// builds a [`SessionSpec`] (`working_dir` inherited from the parent; model/effort
/// from the request, which the webui pre-fills with the parent's current
/// values), and dispatches an [`AdapterCommand::Fork`] to the owning daemon. The
/// child links back to the parent via `SessionMeta::parent_local_id` on its
/// `SessionStarted` (resolved into `parent_id` server-side) — the parent row is
/// left untouched, so reopening an archived session as a fork does not revive or
/// re-flip it. Returns a `command_id` the webui can await like a spawn.
pub async fn fork_session(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(session_id): Path<String>,
    Json(req): Json<cctui_proto::api::ForkRequest>,
) -> Result<(StatusCode, Json<cctui_proto::api::ForkResponse>), AppError> {
    let norm = |s: Option<String>| s.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());

    // Resolve the parent: adapter + machine + cwd. The fork inherits the
    // parent's working directory and adapter; only model/effort/name/prompt can
    // be overridden by the caller.
    let row: Option<(Option<String>, Option<uuid::Uuid>, String)> =
        sqlx::query_as("SELECT adapter_id, machine_uuid, working_dir FROM sessions WHERE id = $1")
            .bind(&session_id)
            .fetch_optional(&state.pool)
            .await?;
    let Some((adapter_id, machine_uuid, working_dir)) = row else {
        return Err(AppError::new(StatusCode::NOT_FOUND, "session not found"));
    };
    let Some(adapter_id) = adapter_id else {
        return Err(AppError::new(
            StatusCode::CONFLICT,
            "session has no adapter (legacy) — cannot fork",
        ));
    };
    let Some(machine_uuid) = machine_uuid else {
        return Err(AppError::new(StatusCode::CONFLICT, "session has no machine — cannot fork"));
    };
    crate::routes::spawn::authorize_launch(&state, &ctx, machine_uuid).await?;
    crate::routes::spawn::refuse_foreign_account(&state, &ctx, &session_id).await?;

    // Subset forks slice the parent's on-disk transcript — a claude
    // primitive only. Codex has no partial-fork mechanism, so reject it here
    // rather than let the daemon silently full-fork.
    if req.extract.is_some() && adapter_id != "claude-code" {
        return Err(AppError::new(
            StatusCode::CONFLICT,
            "partial fork (from/after/selected messages) is only supported for claude sessions",
        ));
    }

    let command_id = uuid::Uuid::new_v4();
    // Pre-mint the child session id for claude (which accepts a caller-supplied
    // `--session-id`) so we can return it and the webui can open the new
    // conversation right away. Codex mints its own thread id, so we
    // don't claim one for it.
    let is_claude = adapter_id == "claude-code";
    let child_session_id = is_claude.then(|| uuid::Uuid::new_v4().to_string());
    let service_tier = if crate::routes::gateway::Family::from_adapter(&adapter_id)
        == Ok(crate::routes::gateway::Family::Openai)
    {
        let account_settings =
            crate::routes::gateway::resolve_session_settings(&state, &session_id).await;
        Some(crate::settings_catalog::codex::resolve_service_tier(None, account_settings.as_ref()))
    } else {
        None
    };
    let spec = cctui_proto::adapter::SessionSpec {
        adapter_id: cctui_proto::adapter::AdapterId::new(&adapter_id),
        working_dir: Some(working_dir),
        prompt: norm(req.prompt),
        name: norm(req.name),
        permission_mode: None,
        effort: norm(req.effort),
        model: norm(req.model),
        service_tier,
        env: std::collections::BTreeMap::new(),
        bootstrap: serde_json::Value::Null,
        parent_local_id: None,
    };
    let frame = cctui_proto::ws::DaemonFrameDown::Command {
        adapter_id: adapter_id.clone(),
        command: Box::new(cctui_proto::adapter::AdapterCommand::Fork {
            parent_local_id: session_id.clone(),
            spec,
            command_id: Some(command_id),
            session_id: child_session_id.clone(),
            extract: req.extract,
        }),
    };
    let child_key = child_session_id.clone().unwrap_or_else(|| command_id.to_string());
    crate::routes::context::inherit_intent(&state.pool, &child_key, &session_id).await;
    crate::routes::spawn::remember_spawner(&state.pool, &child_key, ctx.user_id, None).await;
    state.bus.command_daemon_for_session(machine_uuid, &session_id, frame).await.map_err(
        |err| match err {
            crate::bus::BusError::NoDaemon(_) => {
                AppError::new(StatusCode::SERVICE_UNAVAILABLE, "daemon for that machine is offline")
            }
            _ => AppError::new(StatusCode::SERVICE_UNAVAILABLE, "daemon disconnected mid-dispatch"),
        },
    )?;
    tracing::info!(parent = %session_id, %command_id, %adapter_id, child = ?child_session_id, "fork dispatched");
    events::record(
        &state,
        Event::new(kind::SESSION_FORKED, Actor::User(ctx.user_id))
            .session(&session_id)
            .machine(machine_uuid)
            .detail(serde_json::json!({
                "parent_session_id": session_id,
                "child_session_id": child_session_id,
                "command_id": command_id,
                "adapter_id": adapter_id,
            })),
    );
    Ok((
        StatusCode::ACCEPTED,
        Json(cctui_proto::api::ForkResponse {
            command_id,
            status: "dispatched".into(),
            session_id: child_session_id,
        }),
    ))
}

/// `POST /api/v1/sessions/{id}/auto-approve` — toggle cctui-side auto-approve.
/// When on, incoming permission requests for this session are
/// answered `allow` immediately. In-memory; reset on server restart.
pub async fn set_auto_approve(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(req): Json<cctui_proto::api::AutoApproveRequest>,
) -> Result<StatusCode, AppError> {
    state.permission_store.write().await.set_auto_approve(&session_id, req.enabled);
    tracing::info!(session_id = %session_id, enabled = req.enabled, "auto-approve toggled");
    Ok(StatusCode::NO_CONTENT)
}

/// Archive a session: dismiss it from the default list. Best-effort dispatches
/// `Remove` to the owning daemon — for claude-code that stops the worker and
/// then runs `claude rm` so the session also disappears from Claude Code's
/// native `claude agents` view; for codex it is a plain kill. Then
/// marks the row `archived` and drops it from the live registry. The
/// conversation transcript is preserved. Reversible (cctui-side) via
/// `unarchive_session`, though the underlying claude job is gone by then.
///
/// A pinned session is the human's "do not lose this" mark: it is refused
/// with `409 Conflict` unless the caller passes `?force=true`, which only the
/// webui's deliberate single-row gesture does. Scripts, macros and sweeps
/// never force, so a pin is never undone by an automation.
pub async fn archive_session(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(session_id): Path<String>,
    Query(q): Query<ArchiveQuery>,
) -> Result<StatusCode, AppError> {
    let outcome =
        archive_one(&state, &session_id, q.force, RemoveInitiator::User, Actor::User(ctx.user_id))
            .await?;
    match outcome {
        ArchiveOutcome::Archived => Ok(StatusCode::NO_CONTENT),
        ArchiveOutcome::SkippedPinned => Err(AppError::new(
            StatusCode::CONFLICT,
            "session is pinned; unpin it or pass ?force=true",
        )),
    }
}

/// Query string of `POST /sessions/{id}/archive`.
#[derive(Debug, Default, Deserialize)]
pub struct ArchiveQuery {
    /// Archive even when the session is pinned. Reserved for an explicit human
    /// gesture on that one session; never set by automations.
    #[serde(default)]
    pub force: bool,
}

/// What [`archive_one`] did with a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveOutcome {
    Archived,
    /// The session is pinned and `force` was not set: nothing was touched.
    SkippedPinned,
}

/// Ask the owning daemon to remove `session_id`'s underlying job. Tracked so a
/// refusal surfaces as a `ServerEvent::CommandResult` rather than reading as a
/// clean archive; an undeliverable dispatch leaves the job for the daemon's
/// `ResumeMarks` reconcile to remove on reconnect.
///
/// `initiator` decides whether a claude job cctui did not start is removed at
/// all: only a human's archive may, an automatic sweep leaves it running.
pub async fn dispatch_remove(state: &AppState, session_id: &str, initiator: RemoveInitiator) {
    let command_id = uuid::Uuid::new_v4();
    crate::state::track_command(
        &state.pending_commands,
        command_id,
        Some(session_id.to_owned()),
        None,
    );
    if let Err(err) =
        crate::bus::dispatch(state, session_id, remove_command(session_id, command_id, initiator))
            .await
    {
        state.pending_commands.remove(&command_id);
        tracing::warn!(
            %session_id,
            %err,
            "archive could not reach the owning daemon; job removal deferred to reconcile"
        );
    }
}

/// The `Remove` a [`dispatch_remove`] puts on the wire.
fn remove_command(
    session_id: &str,
    command_id: uuid::Uuid,
    initiator: RemoveInitiator,
) -> cctui_proto::adapter::AdapterCommand {
    cctui_proto::adapter::AdapterCommand::Remove {
        local_id: session_id.to_owned(),
        command_id: Some(command_id),
        initiator,
    }
}

/// Archive a single session (+ its subagents) — the reusable core shared by the
/// single-session route and the batch route. Dispatches `Remove`, marks the row
/// `archived`, clears classifier signals, and drops it from the live registry.
///
/// Pinned sessions are protected: without `force` a pinned session (or a
/// pinned child of the session) is left untouched and the call reports
/// [`ArchiveOutcome::SkippedPinned`]. `force` is the human's explicit
/// single-session gesture; every automation passes `false`.
pub async fn archive_one(
    state: &AppState,
    session_id: &str,
    force: bool,
    initiator: RemoveInitiator,
    actor: Actor,
) -> Result<ArchiveOutcome, sqlx::Error> {
    if !force {
        let pinned: Option<bool> = sqlx::query_scalar("SELECT pinned FROM sessions WHERE id = $1")
            .bind(session_id)
            .fetch_optional(&state.pool)
            .await?;
        if pinned == Some(true) {
            tracing::info!(session_id = %session_id, "archive skipped: session is pinned");
            return Ok(ArchiveOutcome::SkippedPinned);
        }
    }
    // Archive the session AND every descendant nested under it, at any depth: a
    // parent's children should never outlive it in the list. Archiving a
    // *child* does not touch the parent (no `parent_id` cascade upward), and a
    // pinned descendant is never swept along unless forced.
    let children =
        crate::store::sessions::descendants(&state.pool, session_id).await.unwrap_or_default();
    let descendant_ids: Vec<String> = children.iter().map(|c| c.id.clone()).collect();
    // Clear the classifier signals on archive so a session that was waiting on
    // input doesn't keep its ✋ "needs input" glyph in the archived view — an
    // archived session is, by definition, no longer waiting on anyone.
    let archived: Vec<String> = sqlx::query_scalar(
        "UPDATE sessions SET status = 'archived', tempo = NULL, agent_state = NULL, \
                activity = NULL, soft_limit_reason = NULL, archived_by = $4 \
         WHERE (id = $1 OR id = ANY($3)) AND (pinned = false OR $2) \
         RETURNING id",
    )
    .bind(session_id)
    .bind(force)
    .bind(&descendant_ids)
    .bind(initiator.as_str())
    .fetch_all(&state.pool)
    .await?;
    // An archived session is over: its gateway credential dies with it, exactly
    // as it does on a reported end. Without this an archived session keeps a
    // live token, which still counts as load on its account in
    // `in_flight_by_provider` for the whole recent-binding window and skews
    // every election away from an account that is in fact free. The token row
    // survives revoked and still names the account, so an unarchive + resume
    // re-mints a fresh one (see `resolve_session_accounts`).
    for id in &archived {
        crate::routes::gateway::revoke_session_tokens(state, id).await;
    }
    // Deepest first, and always before the parent: a live descendant holds its
    // parent's worktree, and `claude rm` refuses a job whose worktree is the
    // working directory of a live session. Task-tool subagents are observe-only
    // and covered by their parent's `Remove`; CctuiAgent children and forks own
    // a claude job each, which stays in `claude agents` until removed.
    for child in crate::store::sessions::job_children(&children, &archived) {
        dispatch_remove(state, child, initiator).await;
    }
    dispatch_remove(state, session_id, initiator).await;
    let children: Vec<String> =
        children.into_iter().map(|c| c.id).filter(|c| archived.contains(c)).collect();
    {
        let mut registry = state.registry.write().await;
        registry.deregister(session_id);
        for child in &children {
            registry.deregister(child);
        }
    }
    crate::guard_step::forget(session_id);
    for child in &children {
        crate::guard_step::forget(child);
    }
    state.bus.deregister_session_stream(session_id);
    for child in &children {
        state.bus.deregister_session_stream(child);
    }
    tracing::info!(session_id = %session_id, children = children.len(), "session archived");
    if archived.iter().any(|id| id == session_id) {
        events::record(
            state,
            Event::new(kind::SESSION_ARCHIVED, actor).session(session_id).detail(
                serde_json::json!({ "children": children.len(), "initiator": initiator.as_str() }),
            ),
        );
    }
    Ok(ArchiveOutcome::Archived)
}

/// Un-archive a session: clear the sticky `archived` state back to
/// `inactive` so it reappears in the default list and can re-derive its
/// status from activity.
pub async fn unarchive_session(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(session_id): Path<String>,
) -> Result<StatusCode, AppError> {
    unarchive_one(&state, &session_id, ctx.user_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn unarchive_one(
    state: &AppState,
    session_id: &str,
    by: uuid::Uuid,
) -> Result<(), sqlx::Error> {
    if crate::store::sessions::set_inactive(&state.pool, session_id, true).await? {
        tracing::info!(session_id = %session_id, "session unarchived");
        events::record(
            state,
            Event::new(kind::SESSION_UNARCHIVED, Actor::User(by)).session(session_id),
        );
    }
    Ok(())
}

/// Pin (star) a session: it sorts above everything in the live list
/// and is exempt from the auto-archive reaper regardless of heartbeat age.
/// Pinning an already-archived session also un-archives it so it reappears.
pub async fn pin_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<StatusCode, AppError> {
    pin_one(&state, &session_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn pin_one(state: &AppState, session_id: &str) -> Result<(), sqlx::Error> {
    // Pinning un-archives so a pinned session is always visible in the live
    // list. `archived` -> `inactive`; other statuses are left untouched.
    sqlx::query(
        "UPDATE sessions \
         SET pinned = true, pinned_at = now(), \
             status = CASE WHEN status = 'archived' THEN 'inactive' ELSE status END \
         WHERE id = $1",
    )
    .bind(session_id)
    .execute(&state.pool)
    .await?;
    tracing::info!(session_id = %session_id, "session pinned");
    Ok(())
}

/// Unpin a session — returns it to normal recency sorting and makes it eligible
/// for auto-archive again.
pub async fn unpin_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<StatusCode, AppError> {
    unpin_one(&state, &session_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn unpin_one(state: &AppState, session_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE sessions SET pinned = false, pinned_at = NULL WHERE id = $1")
        .bind(session_id)
        .execute(&state.pool)
        .await?;
    tracing::info!(session_id = %session_id, "session unpinned");
    Ok(())
}

/// `POST /api/v1/sessions/{id}/keepalive` — set or clear the prompt-cache
/// keep-alive schedule; answers with the stored schedule (`null` when off).
pub async fn set_keepalive(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(req): Json<cctui_proto::api::SessionKeepaliveRequest>,
) -> Result<Json<Option<cctui_proto::api::KeepaliveState>>, AppError> {
    match crate::keepalive::apply(&state, &session_id, &req).await {
        Ok(Some(Ok(schedule))) => Ok(Json(schedule)),
        Ok(Some(Err(msg))) => Err(AppError::new(StatusCode::BAD_REQUEST, msg)),
        Ok(None) => Err(AppError::new(StatusCode::NOT_FOUND, "session not found")),
        Err(e) => Err(AppError::from(e)),
    }
}

/// `POST /api/v1/sessions/pin` — pin many sessions in one request. Mirrors the
/// batch archive route; per-id failures are logged but don't abort the batch.
// Linear batch loop with per-id error handling; complexity is per-id, not nesting.
#[allow(clippy::cognitive_complexity)]
pub async fn pin_sessions(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<BatchIds>,
) -> StatusCode {
    let ids = match filter_owned_ids(&state, &ctx, &req.ids).await {
        Ok(ids) => ids,
        Err(e) => {
            tracing::error!("db error (batch pin authz): {e}");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    };
    let mut ok = 0usize;
    for id in &ids {
        match pin_one(&state, id).await {
            Ok(()) => ok += 1,
            Err(e) => tracing::error!(session_id = %id, "batch pin db error: {e}"),
        }
    }
    tracing::info!(pinned = ok, requested = req.ids.len(), "batch pin");
    StatusCode::NO_CONTENT
}

/// `POST /api/v1/sessions/unpin` — the batch mirror of `unpin_session`.
// Linear batch loop with per-id error handling; complexity is per-id, not nesting.
#[allow(clippy::cognitive_complexity)]
pub async fn unpin_sessions(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<BatchIds>,
) -> StatusCode {
    let ids = match filter_owned_ids(&state, &ctx, &req.ids).await {
        Ok(ids) => ids,
        Err(e) => {
            tracing::error!("db error (batch unpin authz): {e}");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    };
    let mut ok = 0usize;
    for id in &ids {
        match unpin_one(&state, id).await {
            Ok(()) => ok += 1,
            Err(e) => tracing::error!(session_id = %id, "batch unpin db error: {e}"),
        }
    }
    tracing::info!(unpinned = ok, requested = req.ids.len(), "batch unpin");
    StatusCode::NO_CONTENT
}

/// Body for the batch archive/unarchive routes.
#[derive(Deserialize)]
pub struct BatchIds {
    pub ids: Vec<String>,
}

/// `POST /api/v1/sessions/archive` — archive many sessions in one request
/// (multi-select). Each id is processed via [`archive_one`]; a per-id
/// failure is logged but does not abort the rest of the batch. Idempotent:
/// re-archiving an already-archived id is a no-op.
// Linear batch loop with per-id error handling; complexity is per-id, not nesting.
#[allow(clippy::cognitive_complexity)]
pub async fn archive_sessions(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<BatchIds>,
) -> StatusCode {
    let ids = match filter_owned_ids(&state, &ctx, &req.ids).await {
        Ok(ids) => ids,
        Err(e) => {
            tracing::error!("db error (batch archive authz): {e}");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    };
    // A batch never touches a pinned session: a star means "do not lose this",
    // and a multi-select or a section sweep is exactly where one slips in.
    let mut ok = 0usize;
    let mut pinned = 0usize;
    for id in &ids {
        match archive_one(&state, id, false, RemoveInitiator::User, Actor::User(ctx.user_id)).await
        {
            Ok(ArchiveOutcome::Archived) => ok += 1,
            Ok(ArchiveOutcome::SkippedPinned) => pinned += 1,
            Err(e) => tracing::error!(session_id = %id, "batch archive db error: {e}"),
        }
    }
    tracing::info!(
        archived = ok,
        skipped_pinned = pinned,
        requested = req.ids.len(),
        "batch archive"
    );
    StatusCode::NO_CONTENT
}

/// `POST /api/v1/sessions/unarchive` — the batch mirror of `unarchive_session`.
// Linear batch loop with per-id error handling; complexity is per-id, not nesting.
#[allow(clippy::cognitive_complexity)]
pub async fn unarchive_sessions(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<BatchIds>,
) -> StatusCode {
    let ids = match filter_owned_ids(&state, &ctx, &req.ids).await {
        Ok(ids) => ids,
        Err(e) => {
            tracing::error!("db error (batch unarchive authz): {e}");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    };
    let mut ok = 0usize;
    for id in &ids {
        match unarchive_one(&state, id, ctx.user_id).await {
            Ok(()) => ok += 1,
            Err(e) => tracing::error!(session_id = %id, "batch unarchive db error: {e}"),
        }
    }
    tracing::info!(unarchived = ok, requested = req.ids.len(), "batch unarchive");
    StatusCode::NO_CONTENT
}

pub async fn set_session_policy(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(rules): Json<Vec<crate::policy::PolicyRule>>,
) -> Result<StatusCode, AppError> {
    {
        let mut registry = state.registry.write().await;
        registry.set_policy(&session_id, rules);
    }
    Ok(StatusCode::OK)
}

/// normalize a session's last-message text for the sessions
/// table. Collapses every run of ASCII/Unicode whitespace (newlines,
/// tabs, NBSP) into a single space so multi-line snippets don't blow
/// up row height when CSS doesn't fully suppress wrapping, and caps at
/// 200 chars + ellipsis as a backstop. CSS handles the *visual* truncation
/// to the column's actual rendered width (`text-overflow: ellipsis`).
/// [`normalize_last_message`] over the prose of a SQL-truncated prefix, past
/// the wrappers cctui put ahead of it: `cut` says the stored text ran past it,
/// so the preview is elided even when collapsing whitespace left it under the
/// limit.
fn preview_last_message(prefix: &str, cut: bool) -> String {
    let (_, prose) = cctui_proto::injected::split_injected(prefix);
    let preview = normalize_last_message(if prose.is_empty() { prefix } else { prose });
    if cut && !preview.ends_with('…') { format!("{preview}…") } else { preview }
}

fn normalize_last_message(s: &str) -> String {
    const MAX_CHARS: usize = 200;
    let collapsed = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut iter = collapsed.chars();
    let head: String = iter.by_ref().take(MAX_CHARS).collect();
    if iter.next().is_some() { format!("{head}…") } else { head }
}

/// `PUT /api/v1/sessions/{id}/draft`. Replace a draft's stored spawn payload
/// in place, so an edit or autosave never has to discard and recreate the
/// row. Only `draft` rows are touched; env values are never stored.
pub async fn update_draft(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(session_id): Path<String>,
    Json(req): Json<SpawnRequest>,
) -> Result<Json<SpawnResponse>, AppError> {
    let draft_id = uuid::Uuid::parse_str(&session_id)
        .map_err(|_| AppError::new(StatusCode::BAD_REQUEST, "session id must be a uuid"))?;
    let (machine_uuid, _) = resolve_owned_machine(&state, &ctx, &req.machine_id)
        .await
        .map_err(|(code, Json(e))| AppError::new(code, e.error))?;
    let fields = DraftRowFields::from_request(&req);
    let draft_json = serde_json::to_value(fields.payload).map_err(|e| {
        tracing::error!("serializing draft payload: {e}");
        AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "could not encode draft")
    })?;
    let metadata =
        serde_json::json!({ "draft": draft_json, "draft_saved_at": Utc::now().to_rfc3339() });
    let res = sqlx::query(
        r"UPDATE sessions
          SET machine_id = $2, machine_uuid = $3, working_dir = $4, metadata = $5,
              adapter_id = $6, session_name = $7, model = $8, effort = $9,
              last_heartbeat = now()
          WHERE id = $1 AND status = 'draft'",
    )
    .bind(&session_id)
    .bind(&req.machine_id)
    .bind(machine_uuid)
    .bind(&req.working_dir)
    .bind(&metadata)
    .bind(&fields.adapter_id)
    .bind(fields.name)
    .bind(fields.model)
    .bind(fields.effort)
    .execute(&state.pool)
    .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::new(StatusCode::NOT_FOUND, "draft not found"));
    }
    crate::spawn_labels::sync_draft(&state.pool, &session_id, &req.label_ids, ctx.owner_filter())
        .await;
    tracing::info!(draft = %session_id, "draft updated");
    Ok(Json(SpawnResponse {
        command_id: draft_id,
        status: SessionRowStatus::Draft.as_str().into(),
        account: None,
        session_id: None,
    }))
}

/// The row columns mirrored from a draft payload. Blank strings become NULL
/// so the list renders the row exactly like a freshly saved draft.
struct DraftRowFields<'a> {
    payload: SpawnRequest,
    adapter_id: String,
    name: Option<&'a str>,
    model: Option<&'a str>,
    effort: Option<&'a str>,
}

impl<'a> DraftRowFields<'a> {
    fn from_request(req: &'a SpawnRequest) -> Self {
        let mut payload = req.clone();
        payload.env.clear();
        payload.save_draft = false;
        let non_blank = |v: &'a Option<String>| v.as_deref().filter(|s| !s.trim().is_empty());
        Self {
            payload,
            adapter_id: req.adapter_id.clone().unwrap_or_else(|| "claude-code".to_owned()),
            name: non_blank(&req.name),
            model: non_blank(&req.model),
            effort: non_blank(&req.effort),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Bucket, EndRow, RemoveInitiator, ResultRow, SessionChild, SqlParam, attention_from_bucket,
        bucket_from_signals, cap_unread, compile_node, derive_liveness, field_values_sql,
        make_snippet, normalize_last_message, remove_command, snippet_sql,
    };
    use crate::auth::AuthContext;
    use axum::http::StatusCode;
    use cctui_proto::models::{Attention, Liveness};
    use chrono::{DateTime, Duration, Utc};

    fn bare_session(id: &str) -> cctui_proto::api::SessionListItem {
        serde_json::from_value(serde_json::json!({
            "id": id, "parent_id": null, "machine_id": "m", "working_dir": "/w",
            "status": "active", "metadata": {},
            "token_usage": serde_json::to_value(
                cctui_proto::models::TokenUsage::default()
            ).unwrap(),
        }))
        .unwrap()
    }

    async fn seeded_session(test_name: &str) -> Option<(sqlx::PgPool, String)> {
        let url = crate::routes::gateway::test_db_url(test_name)?;
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let uid = uuid::Uuid::new_v4();
        let machine = uuid::Uuid::new_v4();
        let sid = format!("cct1086-{}", uuid::Uuid::new_v4());
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("u-{uid}"))
            .bind(format!("k-{uid}"))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, 'm', $3)")
            .bind(machine)
            .bind(uid)
            .bind(format!("mk-{machine}"))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status) \
             VALUES ($1, $2, $2, $3, '/w', 'active')",
        )
        .bind(&sid)
        .bind(machine)
        .bind(uid)
        .execute(&pool)
        .await
        .unwrap();
        // Renderable assistant lines interleaved with turn bookkeeping the
        // client never sees; the newest row is bookkeeping, as after any
        // completed turn.
        let payloads = [
            serde_json::json!({"role": "assistant", "text": "one"}),
            serde_json::json!({"role": "no_such_role", "text": "hidden a"}),
            serde_json::json!({"role": "assistant", "text": "two"}),
            serde_json::json!({"role": "assistant", "text": "three"}),
            serde_json::json!({"role": "no_such_role", "text": "hidden b"}),
            serde_json::json!({"role": "assistant", "text": "four"}),
            serde_json::json!({"role": "no_such_role", "text": "hidden c"}),
        ];
        for p in payloads {
            sqlx::query(
                "INSERT INTO stream_events (session_id, event_type, payload) VALUES ($1, 'message', $2)",
            )
            .bind(&sid)
            .bind(p)
            .execute(&pool)
            .await
            .unwrap();
        }
        Some((pool, sid))
    }

    fn renderable(v: serde_json::Value) -> super::RenderableRow {
        (0, v, Utc::now(), None)
    }

    /// The final message is read off newest-first rows: its visible text
    /// rows join in seq order, its thinking rows are skipped, and the first
    /// row of anything else (a tool call, an older message, a user line)
    /// ends it.
    #[test]
    fn final_assistant_text_joins_the_last_message_and_skips_thinking() {
        let newest_first = [
            renderable(serde_json::json!({
                "type": "text", "content": "Session ended: completed", "meta": true,
                "kind": "system_marker",
            })),
            renderable(serde_json::json!({"type": "text", "content": "tail", "message_id": "m2"})),
            renderable(serde_json::json!({
                "type": "text", "content": "mulling", "kind": "thinking", "message_id": "m2",
            })),
            renderable(serde_json::json!({"type": "text", "content": "head", "message_id": "m2"})),
            renderable(serde_json::json!({"type": "tool_call", "tool": "Bash", "input": {}})),
            renderable(serde_json::json!({"type": "text", "content": "draft", "message_id": "m1"})),
            renderable(serde_json::json!({"type": "text", "content": "▷ User: go", "meta": false})),
        ];
        assert_eq!(
            super::final_assistant_text(&newest_first),
            (Some("m2".to_owned()), Some("head\n\ntail".to_owned()))
        );

        let codex = [
            renderable(serde_json::json!({"type": "text", "content": "why", "role": "Reasoning"})),
            renderable(serde_json::json!({"type": "text", "content": "done", "role": "Assistant"})),
            renderable(serde_json::json!({"type": "tool_call", "tool": "shell", "input": {}})),
            renderable(
                serde_json::json!({"type": "text", "content": "first", "role": "Assistant"}),
            ),
        ];
        assert_eq!(super::final_assistant_text(&codex), (None, Some("done".to_owned())));

        let only_user = [renderable(serde_json::json!({"type": "text", "content": "▷ User: hi"}))];
        assert_eq!(super::final_assistant_text(&only_user), (None, None));
        assert_eq!(super::final_assistant_text(&[]), (None, None));
    }

    /// A user with one machine and one live session, plus the state the
    /// result route reads it through.
    async fn result_session(
        test_name: &str,
    ) -> Option<(crate::state::AppState, uuid::Uuid, String)> {
        let url = crate::routes::gateway::test_db_url(test_name)?;
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let uid = uuid::Uuid::new_v4();
        let machine = uuid::Uuid::new_v4();
        let sid = uuid::Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("u-{uid}"))
            .bind(format!("k-{uid}"))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, 'm', $3)")
            .bind(machine)
            .bind(uid)
            .bind(format!("mk-{machine}"))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status) \
             VALUES ($1, $2, $2, $3, '/w', 'active')",
        )
        .bind(&sid)
        .bind(machine)
        .bind(uid)
        .execute(&pool)
        .await
        .unwrap();
        Some((crate::state::AppState::for_test(pool), uid, sid))
    }

    async fn store_rows(pool: &sqlx::PgPool, sid: &str, rows: &[(&str, serde_json::Value)]) {
        for (event_type, payload) in rows {
            sqlx::query(
                "INSERT INTO stream_events (session_id, event_type, payload) VALUES ($1, $2, $3)",
            )
            .bind(sid)
            .bind(*event_type)
            .bind(payload)
            .execute(pool)
            .await
            .unwrap();
        }
    }

    async fn result_of(
        state: &crate::state::AppState,
        sid: &str,
    ) -> Result<cctui_proto::api::SessionResult, crate::error::AppError> {
        super::get_result(axum::extract::State(state.clone()), axum::extract::Path(sid.to_owned()))
            .await
            .map(|j| j.0)
    }

    async fn queued_record(
        pool: &sqlx::PgPool,
        uid: uuid::Uuid,
        sid: &str,
        profile: &str,
        created: &str,
        queued: bool,
    ) {
        sqlx::query(
            "INSERT INTO dispatch_records \
               (session_id, user_id, dispatcher_name, profile, outcome, queued, created_at) \
             VALUES ($1, $2, 'k8s-' || $2::text, $3, 'forwarded', $4, $5::timestamptz)",
        )
        .bind(sid)
        .bind(uid)
        .bind(profile)
        .bind(queued)
        .bind(created)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn a_queued_dispatch_without_a_session_reports_its_queue_position() {
        let Some((state, uid, _)) =
            result_session("a_queued_dispatch_without_a_session_reports_its_queue_position").await
        else {
            return;
        };
        let ids: Vec<String> = (0..4).map(|_| uuid::Uuid::new_v4().to_string()).collect();
        let pool = &state.pool;
        queued_record(pool, uid, &ids[0], "lean", "2026-10-21T10:00:00Z", true).await;
        queued_record(pool, uid, &ids[1], "full", "2026-10-21T10:00:01Z", true).await;
        queued_record(pool, uid, &ids[2], "lean", "2026-10-21T10:00:02Z", true).await;
        queued_record(pool, uid, &ids[3], "lean", "2026-10-21T10:00:03Z", false).await;

        let second = result_of(&state, &ids[2]).await.unwrap();
        assert!(!second.done);
        assert!(second.queued);
        assert_eq!(second.queue_position, Some(2), "the other profile's queue is not counted");
        assert_eq!(result_of(&state, &ids[1]).await.unwrap().queue_position, Some(1));
        assert!(result_of(&state, &ids[3]).await.is_err(), "left the queue: still not found");
        assert!(result_of(&state, &uuid::Uuid::new_v4().to_string()).await.is_err());
    }

    #[tokio::test]
    async fn a_live_session_reports_its_latest_message_as_not_done() {
        let Some((state, _, sid)) =
            result_session("a_live_session_reports_its_latest_message_as_not_done").await
        else {
            return;
        };
        store_rows(
            &state.pool,
            &sid,
            &[
                ("message", serde_json::json!({"role": "user", "text": "go"})),
                ("message", serde_json::json!({"role": "assistant", "text": "draft", "message_id": "m1"})),
                ("tool_use", serde_json::json!({"kind": "tool_use", "name": "Bash", "input": {}})),
                ("message", serde_json::json!({"role": "assistant_thinking", "text": "hmm", "message_id": "m2"})),
                ("message", serde_json::json!({"role": "assistant", "text": "head", "message_id": "m2"})),
                ("message", serde_json::json!({"role": "assistant", "text": "tail", "message_id": "m2"})),
            ],
        )
        .await;
        let result = result_of(&state, &sid).await.unwrap();
        assert!(!result.done);
        assert_eq!(result.end_reason, None);
        assert_eq!(result.message_id.as_deref(), Some("m2"));
        assert_eq!(result.final_assistant_text.as_deref(), Some("head\n\ntail"));
        assert!(result.token_usage.is_none(), "no usage row was stored for m2");
    }

    #[tokio::test]
    async fn an_ended_session_reports_done_with_its_final_text_and_usage() {
        let Some((state, _, sid)) =
            result_session("an_ended_session_reports_done_with_its_final_text_and_usage").await
        else {
            return;
        };
        store_rows(
            &state.pool,
            &sid,
            &[
                ("message", serde_json::json!({"role": "assistant", "text": "old", "message_id": "m1"})),
                ("message", serde_json::json!({"role": "user", "text": "and then?"})),
                ("message", serde_json::json!({"role": "assistant_thinking", "text": "x", "message_id": "m2"})),
                ("message", serde_json::json!({"role": "assistant", "text": "answer", "message_id": "m2"})),
                ("session_ended", serde_json::json!({"reason": {"kind": "completed"}})),
            ],
        )
        .await;
        sqlx::query(
            "INSERT INTO session_token_usage (session_id, message_id, input_tokens, output_tokens) \
             VALUES ($1, 'm2', 120, 7)",
        )
        .bind(&sid)
        .execute(&state.pool)
        .await
        .unwrap();
        sqlx::query("UPDATE sessions SET ended_at = now(), end_reason = 'completed' WHERE id = $1")
            .bind(&sid)
            .execute(&state.pool)
            .await
            .unwrap();
        let result = result_of(&state, &sid).await.unwrap();
        assert!(result.done);
        assert_eq!(result.end_reason.as_deref(), Some("completed"));
        assert_eq!(result.message_id.as_deref(), Some("m2"));
        assert_eq!(result.final_assistant_text.as_deref(), Some("answer"));
        let usage = result.token_usage.expect("usage of m2");
        assert_eq!((usage.tokens_in, usage.tokens_out), (120, 7));
    }

    async fn set_signals(pool: &sqlx::PgPool, sid: &str, sql: &str) {
        sqlx::query(sqlx::AssertSqlSafe(format!("UPDATE sessions SET {sql} WHERE id = $1")))
            .bind(sid)
            .execute(pool)
            .await
            .unwrap();
    }

    async fn store_answer(pool: &sqlx::PgPool, sid: &str) {
        store_rows(
            pool,
            sid,
            &[
                ("message", serde_json::json!({"role": "user", "text": "go"})),
                (
                    "message",
                    serde_json::json!({"role": "assistant", "text": "answer", "message_id": "m1"}),
                ),
            ],
        )
        .await;
    }

    #[tokio::test]
    async fn a_running_turn_is_not_done_even_with_a_stale_success_activity() {
        let Some((state, _, sid)) =
            result_session("a_running_turn_is_not_done_even_with_a_stale_success_activity").await
        else {
            return;
        };
        store_answer(&state.pool, &sid).await;
        set_signals(
            &state.pool,
            &sid,
            "tempo = 'active', agent_state = 'working', activity = 'success'",
        )
        .await;
        let result = result_of(&state, &sid).await.unwrap();
        assert!(!result.done);
        assert_eq!(result.final_assistant_text.as_deref(), Some("answer"));
    }

    #[tokio::test]
    async fn a_finished_turn_on_a_live_session_is_done() {
        let Some((state, _, sid)) =
            result_session("a_finished_turn_on_a_live_session_is_done").await
        else {
            return;
        };
        store_answer(&state.pool, &sid).await;
        set_signals(&state.pool, &sid, "tempo = 'idle', agent_state = 'done'").await;
        let result = result_of(&state, &sid).await.unwrap();
        assert!(result.done);
        assert_eq!(result.end_reason, None);
        assert_eq!(result.message_id.as_deref(), Some("m1"));
        assert_eq!(result.final_assistant_text.as_deref(), Some("answer"));
    }

    #[tokio::test]
    async fn a_turn_waiting_on_input_is_not_done() {
        let Some((state, _, sid)) = result_session("a_turn_waiting_on_input_is_not_done").await
        else {
            return;
        };
        set_signals(&state.pool, &sid, "tempo = 'blocked', agent_state = 'done'").await;
        assert!(!result_of(&state, &sid).await.unwrap().done);
    }

    fn result_row(adapter: &str, tempo: Option<&str>, agent_state: Option<&str>) -> ResultRow {
        ResultRow {
            adapter_id: Some(adapter.to_owned()),
            end_reason: None,
            ended_at: None,
            status: "active".to_owned(),
            tempo: tempo.map(str::to_owned),
            agent_state: agent_state.map(str::to_owned),
            activity: None,
            soft_limit_reason: None,
        }
    }

    #[test]
    fn codex_is_done_once_its_thread_goes_idle_despite_the_stale_active_tempo() {
        assert!(!result_row("codex", Some("active"), Some("working")).done());
        assert!(!result_row("codex", Some("blocked"), Some("working")).done());
        assert!(result_row("codex", Some("active"), Some("idle")).done());
        assert!(result_row("codex", Some("hibernated"), Some("idle")).done());
        assert!(result_row("codex", Some("active"), Some("done")).done());
        assert!(!result_row("codex", None, None).done());
    }

    #[test]
    fn opencode_is_done_once_its_session_goes_idle() {
        assert!(!result_row("opencode", Some("working"), Some("working")).done());
        assert!(!result_row("opencode", Some("error"), Some("error")).done());
        assert!(result_row("opencode", Some("idle"), Some("idle")).done());
        assert!(!result_row("opencode", None, None).done());
    }

    #[test]
    fn an_idle_codex_or_opencode_session_locked_out_by_a_soft_limit_is_not_done() {
        for adapter in ["codex", "opencode"] {
            let mut row = result_row(adapter, Some("idle"), Some("idle"));
            row.soft_limit_reason = Some("switch account".to_owned());
            assert!(!row.done(), "{adapter}");
        }
    }

    #[test]
    fn claude_code_still_reads_the_classifier_bucket() {
        assert!(result_row("claude-code", Some("idle"), Some("done")).done());
        assert!(!result_row("claude-code", Some("idle"), Some("idle")).done());
        assert!(!result_row("claude-code", Some("active"), Some("done")).done());
        let mut ended = result_row("codex", Some("active"), Some("working"));
        ended.ended_at = Some(Utc::now());
        assert!(ended.done());
    }

    #[tokio::test]
    async fn codex_and_opencode_turns_read_done_only_after_going_idle() {
        let turns = [
            ("codex", "tempo = 'active', agent_state = 'working'", "agent_state = 'idle'"),
            (
                "opencode",
                "tempo = 'working', agent_state = 'working'",
                "tempo = 'idle', agent_state = 'idle'",
            ),
        ];
        for (adapter, busy, idle) in turns {
            let Some((state, _, sid)) =
                result_session(&format!("{adapter}_turns_read_done_only_after_going_idle")).await
            else {
                return;
            };
            set_signals(&state.pool, &sid, &format!("adapter_id = '{adapter}', {busy}")).await;
            assert!(!result_of(&state, &sid).await.unwrap().done, "{adapter} busy");
            set_signals(&state.pool, &sid, idle).await;
            assert!(result_of(&state, &sid).await.unwrap().done, "{adapter} idle");
            set_signals(&state.pool, &sid, busy).await;
            assert!(!result_of(&state, &sid).await.unwrap().done, "{adapter} next turn");
        }
    }

    /// Archiving clears the classifier signals and leaves `ended_at` unset,
    /// so a session archived mid-turn is done on its status alone and
    /// reports whatever text it had produced.
    #[tokio::test]
    async fn an_archived_session_is_done_with_the_text_it_had() {
        let Some((state, _, sid)) =
            result_session("an_archived_session_is_done_with_the_text_it_had").await
        else {
            return;
        };
        store_answer(&state.pool, &sid).await;
        set_signals(&state.pool, &sid, "tempo = 'active', agent_state = 'working'").await;
        set_signals(
            &state.pool,
            &sid,
            "status = 'archived', tempo = NULL, agent_state = NULL, activity = NULL, \
             archived_by = 'automatic'",
        )
        .await;
        let result = result_of(&state, &sid).await.unwrap();
        assert!(result.done);
        assert_eq!(result.end_reason, None);
        assert_eq!(result.final_assistant_text.as_deref(), Some("answer"));
    }

    /// The route sits behind the session read guard: the owner reads it,
    /// another user is refused, and an id nobody owns is unknown — the same
    /// 404 the handler itself returns for a session that never registered.
    #[tokio::test]
    async fn the_result_route_is_owner_only_and_unknown_ids_are_404() {
        let Some((state, owner, sid)) =
            result_session("the_result_route_is_owner_only_and_unknown_ids_are_404").await
        else {
            return;
        };
        let other = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(other)
            .bind(format!("u-{other}"))
            .bind(format!("k-{other}"))
            .execute(&state.pool)
            .await
            .unwrap();
        let ctx = |user_id| AuthContext {
            user_id,
            key_id: uuid::Uuid::new_v4(),
            machine_id: None,
            scopes: std::collections::BTreeSet::new(),
        };
        assert_eq!(
            crate::authz::authorize_session_read(&ctx(owner), &sid, &state.pool).await,
            Ok(())
        );
        assert_eq!(
            crate::authz::authorize_session_read(&ctx(other), &sid, &state.pool).await,
            Err(StatusCode::FORBIDDEN)
        );
        let unknown = uuid::Uuid::new_v4().to_string();
        assert_eq!(
            crate::authz::authorize_session_read(&ctx(owner), &unknown, &state.pool).await,
            Err(StatusCode::NOT_FOUND)
        );
        let err = result_of(&state, &unknown).await.expect_err("unknown session");
        assert_eq!(err.status(), StatusCode::NOT_FOUND);

        let empty = result_of(&state, &sid).await.unwrap();
        assert!(!empty.done);
        assert_eq!(empty.final_assistant_text, None);
        assert_eq!(empty.message_id, None);
    }

    fn contents(rows: &[super::RenderableRow]) -> Vec<String> {
        rows.iter().map(|(_, v, _, _)| v["content"].as_str().unwrap().to_owned()).collect()
    }

    /// A session the daemon discovered rather than launched carries the marker
    /// in its metadata; the list's own SELECT -> item mapping has to surface it
    /// as a typed origin, since that is what keeps the TUI's automatic paths off
    /// someone else's `claude --bg` job.
    #[tokio::test]
    async fn a_discovered_job_lists_with_a_foreign_origin() {
        let Some(url) =
            crate::routes::gateway::test_db_url("a_discovered_job_lists_with_a_foreign_origin")
        else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let uid = uuid::Uuid::new_v4();
        let machine = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("u-{uid}"))
            .bind(format!("k-{uid}"))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, 'm', $3)")
            .bind(machine)
            .bind(uid)
            .bind(format!("mk-{machine}"))
            .execute(&pool)
            .await
            .unwrap();

        let cases = [
            (serde_json::json!({"origin": "foreign"}), cctui_proto::api::SessionOrigin::Foreign),
            (serde_json::json!({"origin": "fleet"}), cctui_proto::api::SessionOrigin::Fleet),
            // No marker at all: an older row, or one registered before the
            // daemon published an origin. Ours until proven otherwise.
            (serde_json::json!({}), cctui_proto::api::SessionOrigin::Fleet),
            (serde_json::json!({"origin": "nonsense"}), cctui_proto::api::SessionOrigin::Fleet),
        ];
        for (metadata, want) in cases {
            let sid = format!("cct1266-{}", uuid::Uuid::new_v4());
            sqlx::query(
                "INSERT INTO sessions \
                 (id, machine_id, machine_uuid, user_id, working_dir, status, metadata) \
                 VALUES ($1, $2, $2, $3, '/w', 'active', $4)",
            )
            .bind(&sid)
            .bind(machine)
            .bind(uid)
            .bind(&metadata)
            .execute(&pool)
            .await
            .unwrap();

            let sql = format!("{} WHERE s.id = $1", super::DB_SESSION_SELECT);
            let row = sqlx::query_as::<_, super::DbSession>(sqlx::AssertSqlSafe(sql))
                .bind(&sid)
                .fetch_one(&pool)
                .await
                .unwrap();
            let item = super::db_list_item(row);
            assert_eq!(item.origin, want, "metadata {metadata} should list as {want:?}");
            assert_eq!(item.origin.is_foreign(), want.is_foreign());
        }
    }

    #[tokio::test]
    async fn conversation_page_counts_the_limit_against_renderable_rows() {
        let Some((pool, sid)) =
            seeded_session("conversation_page_counts_the_limit_against_renderable_rows").await
        else {
            return;
        };
        let page = |limit, before, order| {
            let params =
                super::ConversationQuery { limit: Some(limit), before, after: None, order };
            let pool = pool.clone();
            let sid = sid.clone();
            async move { super::renderable_rows(&pool, &sid, "claude-code", &params).await.unwrap() }
        };

        let newest = page(1, None, super::ConversationOrder::Desc).await;
        assert_eq!(contents(&newest), ["four"]);

        let tail = page(3, None, super::ConversationOrder::Desc).await;
        assert_eq!(contents(&tail), ["four", "three", "two"]);

        let rest = page(3, Some(tail.last().unwrap().0), super::ConversationOrder::Desc).await;
        assert_eq!(contents(&rest), ["one"], "a short page means the transcript is exhausted");

        let head = page(2, None, super::ConversationOrder::Asc).await;
        assert_eq!(contents(&head), ["one", "two"]);

        let all = super::renderable_rows(
            &pool,
            &sid,
            "claude-code",
            &super::ConversationQuery::default(),
        )
        .await
        .unwrap();
        assert_eq!(contents(&all), ["four", "three", "two", "one"], "unlimited fetch, query order");
    }

    #[test]
    fn last_message_sql_probes_once_per_session_under_the_partial_index() {
        let sql = super::LAST_MESSAGE_SQL;
        let migration =
            include_str!("../../../../migrations/118_stream_events_latest_message.up.sql");

        assert!(
            migration.contains("WHERE event_type = 'message'"),
            "migration 118 no longer carries the predicate this query is written against"
        );
        assert!(
            sql.contains("event_type = 'message'"),
            "the query must repeat the index predicate character for character"
        );
        assert!(
            sql.contains("JOIN LATERAL") && sql.contains("LIMIT 1"),
            "the per-session LIMIT 1 is what makes each session one index probe"
        );
        assert!(
            !sql.contains("DISTINCT ON"),
            "DISTINCT ON reads every message row of every listed session"
        );
        assert!(
            sql.contains(
                "event_type = 'message' \
                 AND COALESCE(se.payload->>'role', '') NOT IN ('turn_annotation', 'system_marker')"
            ),
            "the role filter must stay a residual ANDed onto the index predicate"
        );
    }

    #[test]
    fn sql_prefix_preview_matches_the_full_text_preview() {
        let n = 400;
        assert!(super::LAST_MESSAGE_SQL.contains(&format!("left(e.body, {n})")));
        assert!(super::LAST_MESSAGE_SQL.contains(&format!("length(e.body) > {n}")));
        let inputs = [
            String::new(),
            "short message".to_owned(),
            "line one\n\n  line two".to_owned(),
            "x".repeat(200),
            "x".repeat(201),
            "é".repeat(400),
            "word ".repeat(80),
            "y".repeat(50_000),
            format!("{}\n{}", "a".repeat(399), "b".repeat(10_000)),
        ];
        for full in inputs {
            let prefix: String = full.chars().take(n).collect();
            let cut = full.chars().count() > n;
            assert_eq!(
                super::preview_last_message(&prefix, cut),
                normalize_last_message(&full),
                "{} chars",
                full.len()
            );
        }
    }

    #[test]
    fn a_spoken_prompt_previews_as_its_utterance() {
        let spoken = crate::voice_live::text::spoken_prompt(
            "why is the sky blue",
            Some(cctui_proto::voice::SpokenStyle::Brief),
        );
        assert_eq!(super::preview_last_message(&spoken, false), "why is the sky blue");
        let only = "<cctui-session-context>\nctx\n</cctui-session-context>";
        assert_eq!(super::preview_last_message(only, false), normalize_last_message(only));
    }

    #[tokio::test]
    async fn last_message_sql_returns_a_bounded_preview_not_the_payload() {
        let Some((pool, sid)) = seeded_session("last_message_sql_bounded_preview").await else {
            return;
        };
        let rows: Vec<super::LastMessageRow> = sqlx::query_as(super::LAST_MESSAGE_SQL)
            .bind(vec![sid.clone()])
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(rows[0].1.as_deref(), Some("hidden c"));
        assert_eq!(rows[0].2, Some(false));

        for (payload, want) in [
            (serde_json::json!({"role": "user", "text": 5, "content": "fallback"}), "fallback"),
            (serde_json::json!({"role": "assistant", "text": "z".repeat(90_000)}), ""),
        ] {
            sqlx::query(
                "INSERT INTO stream_events (session_id, event_type, payload) \
                 VALUES ($1, 'message', $2)",
            )
            .bind(&sid)
            .bind(&payload)
            .execute(&pool)
            .await
            .unwrap();
            let rows: Vec<super::LastMessageRow> = sqlx::query_as(super::LAST_MESSAGE_SQL)
                .bind(vec![sid.clone()])
                .fetch_all(&pool)
                .await
                .unwrap();
            let body = rows[0].1.clone().unwrap();
            if want.is_empty() {
                assert_eq!(body.chars().count(), 400);
                assert_eq!(rows[0].2, Some(true));
            } else {
                assert_eq!(body, want);
            }
        }

        sqlx::query("DELETE FROM sessions WHERE id = $1").bind(&sid).execute(&pool).await.unwrap();
    }

    #[tokio::test]
    async fn turn_annotations_and_markers_never_become_the_preview() {
        let Some((pool, sid)) = seeded_session("last_message_skips_annotations").await else {
            return;
        };
        let (pool, sid) = (&pool, &sid);
        let insert = move |payload: serde_json::Value| async move {
            sqlx::query(
                "INSERT INTO stream_events (session_id, event_type, payload) \
                 VALUES ($1, 'message', $2)",
            )
            .bind(sid)
            .bind(payload)
            .execute(pool)
            .await
            .unwrap();
        };
        let preview = move || async move {
            let rows: Vec<super::LastMessageRow> = sqlx::query_as(super::LAST_MESSAGE_SQL)
                .bind(vec![sid.clone()])
                .fetch_all(pool)
                .await
                .unwrap();
            rows[0].1.clone()
        };
        let annotation = |name: &str, text: &str| serde_json::json!({"role": "turn_annotation", "annotation": name, "text": text});
        let marker = |name: &str, text: &str| serde_json::json!({"role": "system_marker", "marker": name, "text": text});
        for payload in [
            serde_json::json!({"role": "assistant", "text": "the real reply"}),
            annotation("turn_duration", "turn_duration:3752"),
            annotation("stop_hook_summary", "stop_hook_summary"),
            annotation("file_history", "file_history:snapshot"),
            marker("permission-mode", "mode: normal"),
            marker("queue-operation", "enqueue: next"),
        ] {
            insert(payload).await;
        }
        assert_eq!(preview().await.as_deref(), Some("the real reply"));

        insert(serde_json::json!({"id": "c1", "type": "agentMessage", "text": "No findings."}))
            .await;
        assert_eq!(preview().await.as_deref(), Some("No findings."), "a role-less codex reply");
        insert(annotation("turn_duration", "turn_duration:12")).await;
        assert_eq!(preview().await.as_deref(), Some("No findings."));

        sqlx::query("DELETE FROM sessions WHERE id = $1").bind(sid).execute(pool).await.unwrap();
    }

    #[test]
    fn transcript_hits_attach_seq_and_leave_id_only_matches_none() {
        let mut sessions = vec![bare_session("s-transcript"), bare_session("s-id-only")];
        let rows =
            vec![("s-transcript".to_owned(), "the quick brown fox jumps".to_owned(), 4242_i64)];

        super::attach_transcript_hits(&mut sessions, rows, &["brown".to_owned()]);

        assert_eq!(sessions[0].match_seq, Some(4242));
        assert!(sessions[0].match_snippet.as_deref().unwrap().contains("brown"));

        assert_eq!(sessions[1].match_seq, None);
        assert_eq!(sessions[1].match_snippet, None);
    }

    #[tokio::test]
    async fn context_counts_cover_delivered_rules_and_held_memories_only() {
        let Some(url) = crate::routes::gateway::test_db_url("context_counts_cover_delivered")
        else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let uid = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("u-{uid}"))
            .bind(format!("k-{uid}"))
            .execute(&pool)
            .await
            .unwrap();
        let mut ids = Vec::new();
        for kind in ["memory", "memory", "prompt"] {
            let id: uuid::Uuid = sqlx::query_scalar(
                "INSERT INTO context_items (user_id, kind, name, title) \
                 VALUES ($1, $2, $3, 't') RETURNING id",
            )
            .bind(uid)
            .bind(kind)
            .bind(format!("n-{}", uuid::Uuid::new_v4()))
            .fetch_one(&pool)
            .await
            .unwrap();
            ids.push(id);
        }
        let (full, rules_only, empty, absent) = (
            format!("cc-full-{uid}"),
            format!("cc-rules-{uid}"),
            format!("cc-empty-{uid}"),
            format!("cc-absent-{uid}"),
        );
        for (key, item_ids, rules) in [
            (&full, ids.clone(), serde_json::json!([{}, {}])),
            (&rules_only, Vec::new(), serde_json::json!([{}])),
            (&empty, vec![ids[2]], serde_json::json!([])),
        ] {
            sqlx::query(
                "INSERT INTO session_context (spawn_key, item_ids, rules) VALUES ($1, $2, $3)",
            )
            .bind(key)
            .bind(&item_ids)
            .bind(sqlx::types::Json(rules))
            .execute(&pool)
            .await
            .unwrap();
        }

        let counts = super::fetch_context_counts(
            &pool,
            &[full.clone(), rules_only.clone(), empty.clone(), absent.clone()],
        )
        .await
        .unwrap();

        let c = |rules, memories| cctui_proto::api::ContextCounts { rules, memories };
        assert_eq!(counts.get(&full), Some(&c(2, 2)), "prompts are not counted");
        assert_eq!(counts.get(&rules_only), Some(&c(1, 0)));
        assert_eq!(counts.get(&empty), None, "nothing to show is left out");
        assert_eq!(counts.get(&absent), None);

        sqlx::query("DELETE FROM session_context WHERE spawn_key = ANY($1)")
            .bind(vec![full, rules_only, empty])
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.unwrap();
    }

    type TotalsRow = (String, Option<String>, Option<i64>, Option<i64>, Option<i64>, Option<i64>);

    async fn recomputed_totals(pool: &sqlx::PgPool, sid: &str) -> Vec<TotalsRow> {
        let mut rows: Vec<TotalsRow> = sqlx::query_as(
            "SELECT session_id, model, \
                    SUM(input_tokens)::bigint, SUM(output_tokens)::bigint, \
                    SUM(cache_read_tokens)::bigint, SUM(cache_creation_tokens)::bigint \
             FROM session_token_usage WHERE session_id = ANY($1) \
             GROUP BY session_id, model",
        )
        .bind(vec![sid.to_owned()])
        .fetch_all(pool)
        .await
        .unwrap();
        rows.sort();
        rows
    }

    async fn trigger_totals(pool: &sqlx::PgPool, sid: &str) -> Vec<TotalsRow> {
        let mut rows: Vec<TotalsRow> = sqlx::query_as(super::SESSION_TOTALS_SQL)
            .bind(vec![sid.to_owned()])
            .fetch_all(pool)
            .await
            .unwrap();
        rows.retain(|r| r.2 != Some(0) || r.3 != Some(0) || r.4 != Some(0) || r.5 != Some(0));
        rows.sort();
        rows
    }

    #[tokio::test]
    async fn trigger_totals_equal_a_recomputed_sum() {
        let Some((pool, sid)) = seeded_session("trigger_totals_equal_a_recomputed_sum").await
        else {
            return;
        };
        let insert = |mid: &'static str, model: Option<&'static str>, n: i64| {
            let pool = pool.clone();
            let sid = sid.clone();
            async move {
                sqlx::query(
                    "INSERT INTO session_token_usage (session_id, message_id, model, \
                     input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens) \
                     VALUES ($1, $2, $3, $4, $4 * 2, $4 * 3, $4 * 4) \
                     ON CONFLICT (session_id, message_id) DO NOTHING",
                )
                .bind(&sid)
                .bind(mid)
                .bind(model)
                .bind(n)
                .execute(&pool)
                .await
                .unwrap();
            }
        };
        insert("a", Some("opus"), 1).await;
        insert("b", Some("opus"), 10).await;
        insert("c", Some("haiku"), 100).await;
        insert("d", None, 1000).await;
        insert("e", None, 5).await;
        insert("b", Some("opus"), 999_999).await;
        assert_eq!(trigger_totals(&pool, &sid).await, recomputed_totals(&pool, &sid).await);

        sqlx::query(
            "UPDATE session_token_usage SET model = 'haiku', input_tokens = 7 \
             WHERE session_id = $1 AND message_id = 'a'",
        )
        .bind(&sid)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("DELETE FROM session_token_usage WHERE session_id = $1 AND message_id = 'd'")
            .bind(&sid)
            .execute(&pool)
            .await
            .unwrap();
        let totals = trigger_totals(&pool, &sid).await;
        assert_eq!(totals, recomputed_totals(&pool, &sid).await);
        assert!(totals.iter().any(|r| r.1.is_none()), "NULL-model rows round-trip as None");

        sqlx::query("DELETE FROM sessions WHERE id = $1").bind(&sid).execute(&pool).await.unwrap();
        let left: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM session_usage_totals WHERE session_id = $1")
                .bind(&sid)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(left, 0);
    }

    #[tokio::test]
    async fn last_two_turns_are_the_newest_two_per_session() {
        let Some((pool, sid)) = seeded_session("last_two_turns_newest_two").await else {
            return;
        };
        let t0 = Utc::now() - Duration::hours(1);
        for n in 0..5_i64 {
            sqlx::query(
                "INSERT INTO session_token_usage (session_id, message_id, input_tokens, created_at) \
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(&sid)
            .bind(format!("t{n}"))
            .bind(n)
            .bind(t0 + Duration::seconds(n))
            .execute(&pool)
            .await
            .unwrap();
        }
        let rows: Vec<(String, i64, i64, i64, chrono::DateTime<Utc>, i64)> =
            sqlx::query_as(super::LAST_TWO_TURNS_SQL)
                .bind(vec![sid.clone(), "no-such-session".to_owned()])
                .fetch_all(&pool)
                .await
                .unwrap();
        let got: Vec<(i64, i64)> = rows.iter().map(|r| (r.1, r.5)).collect();
        assert_eq!(got, [(4, 1), (3, 2)]);

        sqlx::query("DELETE FROM sessions WHERE id = $1").bind(&sid).execute(&pool).await.unwrap();
    }

    #[tokio::test]
    async fn unread_count_stops_at_the_cap() {
        let Some((pool, sid)) = seeded_session("unread_count_stops_at_the_cap").await else {
            return;
        };
        let uid: uuid::Uuid = sqlx::query_scalar("SELECT user_id FROM sessions WHERE id = $1")
            .bind(&sid)
            .fetch_one(&pool)
            .await
            .unwrap();
        let unread = || {
            let pool = pool.clone();
            let sid = sid.clone();
            async move {
                let rows: Vec<(String, i64)> = sqlx::query_as(super::UNREAD_COUNT_SQL)
                    .bind(vec![sid.clone()])
                    .bind(uid)
                    .fetch_all(&pool)
                    .await
                    .unwrap();
                assert_eq!(rows.len(), 1);
                rows[0].1
            }
        };
        assert_eq!(unread().await, 7, "never read: every message of the fixture");

        sqlx::query(
            "INSERT INTO stream_events (session_id, event_type, payload) \
             SELECT $1, 'message', jsonb_build_object('role', 'assistant', 'text', g::text) \
             FROM generate_series(1, 150) g",
        )
        .bind(&sid)
        .execute(&pool)
        .await
        .unwrap();
        let n = unread().await;
        assert_eq!(n, 100);
        assert_eq!(super::cap_unread(n), 99);

        sqlx::query(
            "INSERT INTO session_reads (session_id, user_id, last_seen_at) \
             VALUES ($1, $2, now() + interval '1 minute')",
        )
        .bind(&sid)
        .bind(uid)
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(unread().await, 0);

        sqlx::query("DELETE FROM sessions WHERE id = $1").bind(&sid).execute(&pool).await.unwrap();
    }

    #[tokio::test]
    async fn page_usage_reads_the_page_span_plus_a_bounded_lookback() {
        let Some((pool, sid)) = seeded_session("page_usage_bounded").await else {
            return;
        };
        let t0 = Utc::now() - Duration::hours(3);
        for n in 0..200_i64 {
            sqlx::query(
                "INSERT INTO session_token_usage (session_id, message_id, input_tokens, \
                 cache_read_tokens, created_at) VALUES ($1, $2, 10, $3, $4)",
            )
            .bind(&sid)
            .bind(format!("m{n:03}"))
            .bind(n * 100)
            .bind(t0 + Duration::seconds(n))
            .execute(&pool)
            .await
            .unwrap();
        }
        let page: Vec<String> = (150..160).map(|n| format!("m{n:03}")).collect();

        let rows = super::page_usage_rows(&pool, &sid, &page).await.unwrap();

        let limit = page.len() + usize::try_from(super::USAGE_LOOKBACK).unwrap();
        assert_eq!(rows.len(), limit, "page span plus lookback, not the whole session");
        let ids: std::collections::HashSet<&str> = rows.iter().map(|r| r.0.as_str()).collect();
        assert!(page.iter().all(|m| ids.contains(m.as_str())));
        assert!(ids.contains("m149"), "the previous turn of the stream is read");
        assert!(!ids.contains("m000") && !ids.contains("m199"));
        assert!(super::page_usage_rows(&pool, &sid, &[]).await.unwrap().is_empty());

        sqlx::query("DELETE FROM sessions WHERE id = $1").bind(&sid).execute(&pool).await.unwrap();
    }

    fn conv_filters_of(q: &str) -> super::ConvSearchFilters {
        let root = cctui_query::parse(q);
        let (mut terms, mut raw) = (Vec::new(), Vec::new());
        super::flatten_conv_query(&root, false, &mut terms, &mut raw);
        super::conv_filters(&raw)
    }

    fn conv_terms_of(q: &str) -> Vec<String> {
        let root = cctui_query::parse(q);
        let (mut terms, mut raw) = (Vec::new(), Vec::new());
        super::flatten_conv_query(&root, false, &mut terms, &mut raw);
        terms
    }

    #[test]
    fn conversation_search_ands_every_term_over_the_session_chunks() {
        let sql = super::conv_search_sql(3);
        assert_eq!(sql.matches("c.body ILIKE $").count(), 3);
        assert!(sql.contains("$2") && sql.contains("$3") && sql.contains("$4"));
        assert_eq!(sql.matches(" INTERSECT ").count(), 2, "terms are ANDed per event: {sql}");
        assert_eq!(sql.matches("c.session_id = $1").count(), 3, "every probe stays in the session");
        assert!(!sql.contains("search_text"), "{sql}");
        assert!(sql.contains("WHERE e.session_id = $1"), "the hit list never leaves the session");
        assert!(sql.ends_with("ORDER BY e.id ASC LIMIT $7"), "oldest first: {sql}");

        let none = super::conv_search_sql(0);
        assert!(none.contains("AND (TRUE)"));
        assert!(none.ends_with("ORDER BY e.id ASC LIMIT $4"));
    }

    #[test]
    fn conversation_search_reads_role_tool_and_date_clauses() {
        let f = conv_filters_of("role:user foo");
        assert_eq!(f.roles, ["user"]);
        assert_eq!(conv_terms_of("role:user foo"), ["foo"]);

        let f = conv_filters_of("type:tool,result rm");
        assert_eq!(f.roles, ["tool", "result"]);

        let f = conv_filters_of("-role:thinking");
        assert_eq!(f.not_roles, ["thinking"]);
        assert!(f.roles.is_empty());

        let f = conv_filters_of("tool:Bash rm");
        assert_eq!(f.tools, ["Bash"]);
        assert_eq!(conv_terms_of("tool:Bash rm"), ["rm"]);

        let f = conv_filters_of("after:2026-09-01 before:2026-09-30");
        assert_eq!(f.after.unwrap().to_rfc3339(), "2026-09-01T00:00:00+00:00");
        assert_eq!(f.before.unwrap().to_rfc3339(), "2026-09-30T00:00:00+00:00");

        assert_eq!(conv_filters_of("pinned:true").pinned, Some(true));
        assert_eq!(conv_filters_of("pinned:false").pinned, Some(false));
        assert_eq!(conv_filters_of("foo").pinned, None);
    }

    #[test]
    fn conversation_search_filters_accept_only_matching_events() {
        let pins: std::collections::HashSet<i64> = std::iter::once(7).collect();

        let f = conv_filters_of("role:user");
        assert!(f.accepts("user", None, 1, &pins));
        assert!(!f.accepts("assistant", None, 1, &pins));

        let f = conv_filters_of("tool:mcp__github__*");
        assert!(f.accepts("mcp", Some("mcp__github__create_issue"), 1, &pins));
        assert!(!f.accepts("tool", Some("Bash"), 1, &pins));

        let f = conv_filters_of("tool:bash");
        assert!(f.accepts("tool", Some("Bash"), 1, &pins), "tool ids match case-insensitively");

        let f = conv_filters_of("pinned:true");
        assert!(f.accepts("assistant", None, 7, &pins));
        assert!(!f.accepts("assistant", None, 8, &pins));

        assert!(
            !conv_filters_of("foo").accepts("", None, 1, &pins),
            "a row that normalises to nothing renderable is never a hit"
        );
    }

    async fn index_session(pool: &sqlx::PgPool, sid: &str) {
        let ids: Vec<i64> =
            sqlx::query_scalar("SELECT id FROM stream_events WHERE session_id = $1")
                .bind(sid)
                .fetch_all(pool)
                .await
                .unwrap();
        crate::search_index::reindex(pool, &ids).await.unwrap();
    }

    async fn insert_payload(
        pool: &sqlx::PgPool,
        sid: &str,
        kind: &str,
        payload: serde_json::Value,
    ) {
        sqlx::query(
            "INSERT INTO stream_events (session_id, event_type, payload) VALUES ($1, $2, $3)",
        )
        .bind(sid)
        .bind(kind)
        .bind(payload)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn conversation_search_matches_text_copied_from_the_rendered_view() {
        let Some((pool, sid)) = seeded_session("conversation_search_rendered").await else {
            return;
        };
        let say = |text: String| serde_json::json!({"role": "assistant", "text": text});
        insert_payload(
            &pool,
            &sid,
            "message",
            say("Change the gate to `master...HEAD` (three dots).".into()),
        )
        .await;
        insert_payload(
            &pool,
            &sid,
            "message",
            say(format!("{} the hidden tail phrase", "filler ".repeat(2000))),
        )
        .await;
        insert_payload(
            &pool,
            &sid,
            "message",
            say("It\u{2019}s \u{201C}quoted\u{201D} text".into()),
        )
        .await;
        insert_payload(
            &pool,
            &sid,
            "message",
            say("first line\nsecond **line**\n\n- third".into()),
        )
        .await;
        let blob = "iVBORw0KGgoAAAANSUhEUgAA".repeat(40);
        insert_payload(
            &pool,
            &sid,
            "tool_use",
            serde_json::json!({"kind": "tool_result", "content": [
                {"type": "text", "text": "screenshot saved"},
                {"type": "image", "source": {"type": "base64", "data": blob}},
            ]}),
        )
        .await;
        index_session(&pool, &sid).await;

        let hits = |q: &'static str| {
            let (pool, sid) = (pool.clone(), sid.clone());
            async move {
                let root = cctui_query::parse(q);
                let terms = super::fold_terms(&root.free_text_terms());
                super::conv_search_rows(&pool, &sid, &terms, &super::conv_filters(&[]))
                    .await
                    .unwrap()
                    .expect("no timeout")
                    .len()
            }
        };
        assert_eq!(hits("\"Change the gate to master...HEAD (three dots).\"").await, 1);
        assert_eq!(hits("\"the hidden tail phrase\"").await, 1, "text past 8 KB");
        assert_eq!(hits("\"It's \"").await, 1, "a straight quote finds a smart one");
        assert_eq!(hits("\"first line second line third\"").await, 1, "multi-line copy");
        assert_eq!(hits("screenshot saved").await, 1);
        assert_eq!(hits("iVBORw0KGgo").await, 0, "image payloads are not indexed");
        assert_eq!(hits("gate dots").await, 1, "unquoted words must all appear");
        assert_eq!(hits("gate absentword").await, 0);

        let page = super::SearchPage { limit: 10, offset: 0, cursor: None, uid: None };
        let root = cctui_query::parse("\"master...HEAD (three dots)\"");
        let found = super::scan_matches(&pool, &root, None, &page).await.unwrap();
        assert!(found.rows.iter().any(|r| r.id == sid), "the session list finds it too");

        sqlx::query("DELETE FROM sessions WHERE id = $1").bind(&sid).execute(&pool).await.unwrap();
    }

    #[test]
    fn search_cursor_round_trips() {
        let at = "2026-10-21T10:11:12.345678Z".parse::<DateTime<Utc>>().unwrap();
        let raw = super::search_cursor(at, "cct:with:colons");
        assert_eq!(super::parse_search_cursor(&raw), Some((at, "cct:with:colons".to_owned())));
        assert_eq!(super::parse_search_cursor("nope"), None);
        assert_eq!(super::parse_search_cursor("12:"), None);
    }

    #[tokio::test]
    async fn conversation_search_sql_ands_terms_and_honours_the_date_window() {
        let Some((pool, sid)) = seeded_session("conversation_search_sql").await else {
            return;
        };
        for (text, at) in [
            ("alpha and beta together", "2026-09-05T00:00:00Z"),
            ("alpha only", "2026-09-05T00:00:00Z"),
            ("alpha and beta, but later", "2026-10-05T00:00:00Z"),
        ] {
            sqlx::query(
                "INSERT INTO stream_events (session_id, event_type, payload, created_at) \
                 VALUES ($1, 'message', jsonb_build_object('role', 'assistant', 'text', $2::text), $3)",
            )
            .bind(&sid)
            .bind(text)
            .bind(at.parse::<DateTime<Utc>>().unwrap())
            .execute(&pool)
            .await
            .unwrap();
        }

        index_session(&pool, &sid).await;
        let run =
            |patterns: Vec<String>, after: Option<DateTime<Utc>>, before: Option<DateTime<Utc>>| {
                let (pool, sid) = (pool.clone(), sid.clone());
                async move {
                    let mut q = sqlx::query_as::<_, super::ConvSearchRow>(sqlx::AssertSqlSafe(
                        super::conv_search_sql(patterns.len()),
                    ))
                    .bind(&sid);
                    for p in &patterns {
                        q = q.bind(p);
                    }
                    q.bind(after).bind(before).bind(100_i64).fetch_all(&pool).await.unwrap()
                }
            };
        let texts = |rows: Vec<super::ConvSearchRow>| {
            rows.into_iter()
                .map(|r| r.2["text"].as_str().unwrap_or_default().to_owned())
                .collect::<Vec<_>>()
        };

        let both = texts(run(vec!["%alpha%".into(), "%beta%".into()], None, None).await);
        assert_eq!(both.len(), 2, "only events carrying BOTH terms: {both:?}");
        assert!(both.iter().all(|t| t.contains("alpha") && t.contains("beta")));

        let one = run(vec!["%alpha%".into()], None, None).await;
        assert_eq!(one.len(), 3);
        let ids: Vec<i64> = one.iter().map(|r| r.0).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        assert_eq!(ids, sorted, "hits come back oldest first");

        let windowed = run(
            vec!["%alpha%".into()],
            None,
            Some("2026-10-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap()),
        )
        .await;
        assert_eq!(windowed.len(), 2, "`before` is exclusive of the bound");

        let after = run(
            vec!["%alpha%".into()],
            Some("2026-10-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap()),
            None,
        )
        .await;
        assert_eq!(after.len(), 1);

        sqlx::query("DELETE FROM sessions WHERE id = $1").bind(&sid).execute(&pool).await.unwrap();
    }

    #[test]
    fn conversation_pages_seek_the_session_id_index() {
        let migration = include_str!("../../../../migrations/137_stream_events_session_id.up.sql");
        assert!(migration.contains("ON stream_events (session_id, id)"));
        for order in [super::ConversationOrder::Desc, super::ConversationOrder::Asc] {
            let sql = super::conversation_sql(order);
            assert!(sql.contains("WHERE session_id = $1") && sql.contains("ORDER BY id "));
        }
    }

    #[test]
    fn conversation_order_defaults_to_desc() {
        let q: super::ConversationQuery =
            serde_json::from_value(serde_json::json!({"limit": 20})).unwrap();
        assert_eq!(q.order, super::ConversationOrder::Desc);
        assert!(super::conversation_sql(q.order).ends_with("ORDER BY id DESC LIMIT $3"));
    }

    #[test]
    fn conversation_order_asc_flips_the_inner_order_by() {
        let q: super::ConversationQuery =
            serde_json::from_value(serde_json::json!({"limit": 20, "order": "asc"})).unwrap();
        assert_eq!(q.order, super::ConversationOrder::Asc);
        assert!(super::conversation_sql(q.order).ends_with("ORDER BY id ASC LIMIT $3"));
        let desc = super::conversation_sql(super::ConversationOrder::Desc);
        assert_eq!(
            super::conversation_sql(super::ConversationOrder::Asc).replace("id ASC", "id DESC"),
            desc,
            "asc/desc must differ only in the ORDER BY direction"
        );
    }

    #[test]
    fn draft_row_fields_strip_env_and_blank_columns() {
        let req: super::SpawnRequest = serde_json::from_value(serde_json::json!({
            "machine_id": "m", "working_dir": "/w", "prompt": "hi", "prompt_name": null,
            "name": "  ", "model": "opus", "effort": "",
            "env": {"TOKEN": "secret"}, "env_keys": ["TOKEN"], "save_draft": true
        }))
        .unwrap();
        let f = super::DraftRowFields::from_request(&req);
        assert_eq!(f.adapter_id, "claude-code");
        assert_eq!(f.name, None);
        assert_eq!(f.model, Some("opus"));
        assert_eq!(f.effort, None);
        assert!(f.payload.env.is_empty());
        assert!(!f.payload.save_draft);
        assert_eq!(f.payload.env_keys, vec!["TOKEN"]);
        let json = serde_json::to_value(&f.payload).unwrap();
        assert!(json.get("env").is_none(), "{json}");
    }

    #[test]
    fn oversize_query_is_bad_request() {
        assert!(super::check_query_len(&"a".repeat(cctui_query::MAX_QUERY_LEN)).is_ok());
        let err = super::check_query_len(&"(".repeat(20_000)).unwrap_err();
        assert!(matches!(err, super::AppError::Status(axum::http::StatusCode::BAD_REQUEST, _)));
    }

    #[test]
    fn compile_node_is_depth_bounded() {
        let mut node = cctui_query::Node::Text { value: "x".into() };
        for _ in 0..100_000 {
            node = cctui_query::Node::Not { child: Box::new(node) };
        }
        let mut params = Vec::new();
        let sql = std::thread::Builder::new()
            .stack_size(1024 * 1024)
            .spawn(move || {
                let sql = compile_node(&node, &mut params);
                std::mem::forget(node);
                sql
            })
            .unwrap()
            .join()
            .unwrap();
        assert!(sql.contains("FALSE"));
    }

    fn compile(q: &str) -> (String, Vec<String>) {
        let mut params = Vec::new();
        let sql = compile_node(&cctui_query::parse(q), &mut params);
        let texts = params
            .into_iter()
            .map(|p| match p {
                SqlParam::Text(s) => s,
                SqlParam::Bool(b) => b.to_string(),
            })
            .collect();
        (sql, texts)
    }

    #[test]
    fn field_values_empty_context_is_context_free() {
        let (sql, ctx) = field_values_sql("model", "").expect("model is dynamic");
        assert!(ctx.is_empty());
        assert!(sql.contains("WHERE (TRUE) AND"), "{sql}");
        assert!(sql.contains("$1::uuid IS NULL"), "{sql}");
        assert!(sql.contains("$2::text IS NULL"), "{sql}");
    }

    #[test]
    fn field_values_context_ands_in_and_shifts_binds() {
        let (sql, ctx) = field_values_sql("model", "account=acctbot").expect("model is dynamic");
        assert_eq!(ctx.len(), 1);
        match &ctx[0] {
            SqlParam::Text(s) => assert_eq!(s, "acctbot"),
            SqlParam::Bool(_) => panic!("expected text bind"),
        }
        assert!(sql.contains("account_providers"), "context predicate present:\n{sql}");
        assert!(sql.contains("lower(${1}") || sql.contains("lower($1"), "{sql}");
        assert!(sql.contains("$2::uuid IS NULL"), "{sql}");
        assert!(sql.contains("$3::text IS NULL"), "{sql}");
    }

    #[test]
    fn field_values_unknown_field_is_none() {
        assert!(field_values_sql("nope", "").is_none());
    }

    fn ts(secs: i64) -> chrono::DateTime<Utc> {
        chrono::DateTime::from_timestamp(1_700_000_000 + secs, 0).unwrap()
    }

    #[test]
    fn sort_pins_first_then_last_message_falling_back_to_registration() {
        struct Case {
            id: &'static str,
            pinned: bool,
            registered: i64,
            last_message: Option<i64>,
        }
        let cases = [
            Case { id: "old-unpinned", pinned: false, registered: 10, last_message: None },
            Case { id: "pinned-quiet", pinned: true, registered: 5, last_message: None },
            Case { id: "newest-msg", pinned: false, registered: 1, last_message: Some(100) },
            Case { id: "pinned-chatty", pinned: true, registered: 2, last_message: Some(50) },
            Case { id: "registered-late", pinned: false, registered: 60, last_message: None },
            Case { id: "stale-msg", pinned: false, registered: 70, last_message: Some(20) },
        ];
        let with_ts = cases
            .iter()
            .map(|c| {
                let mut s = bare_session(c.id);
                s.pinned = c.pinned;
                s.last_message_at = c.last_message.map(ts);
                (ts(c.registered), s)
            })
            .collect();
        let ids: Vec<String> = super::sort_sessions(with_ts).into_iter().map(|s| s.id).collect();
        assert_eq!(
            ids,
            [
                "pinned-chatty",
                "pinned-quiet",
                "newest-msg",
                "registered-late",
                "stale-msg",
                "old-unpinned"
            ]
        );
    }

    #[test]
    fn sort_of_nothing_is_nothing() {
        assert!(super::sort_sessions(Vec::new()).is_empty());
    }

    #[test]
    fn last_turn_cache_signals() {
        struct Case {
            name: &'static str,
            cache_read: i64,
            cache_creation: i64,
            prev_context: i64,
            cold: bool,
            burst: Option<u64>,
        }
        let cases = [
            Case {
                name: "read back most of the previous context",
                cache_read: 800,
                cache_creation: 50,
                prev_context: 1000,
                cold: false,
                burst: Some(850),
            },
            Case {
                name: "read back under half the previous context",
                cache_read: 400,
                cache_creation: 600,
                prev_context: 1000,
                cold: true,
                burst: Some(1000),
            },
            Case {
                name: "exactly half is not cold",
                cache_read: 500,
                cache_creation: 0,
                prev_context: 1000,
                cold: false,
                burst: Some(500),
            },
            Case {
                name: "first turn that only wrote cache",
                cache_read: 0,
                cache_creation: 300,
                prev_context: 0,
                cold: true,
                burst: Some(300),
            },
            Case {
                name: "first turn that read something",
                cache_read: 10,
                cache_creation: 300,
                prev_context: 0,
                cold: false,
                burst: Some(310),
            },
            Case {
                name: "first turn with no cache activity",
                cache_read: 0,
                cache_creation: 0,
                prev_context: 0,
                cold: false,
                burst: None,
            },
            Case {
                name: "negative counters clamp",
                cache_read: -5,
                cache_creation: -5,
                prev_context: 100,
                cold: true,
                burst: None,
            },
        ];
        for c in cases {
            let turn = super::LastTurn {
                cache_read: c.cache_read,
                cache_creation: c.cache_creation,
                at: ts(0),
                prev_context: c.prev_context,
            };
            assert_eq!(turn.cache_cold(), c.cold, "{}", c.name);
            assert_eq!(turn.estimated_burst_tokens(), c.burst, "{}", c.name);
        }
    }

    #[test]
    fn fold_last_turns_pairs_newest_with_previous_context() {
        let rows = vec![
            ("a".to_string(), 100, 200, 300, ts(9), 1),
            ("a".to_string(), 10, 20, 30, ts(8), 2),
            ("b".to_string(), 1, 2, 3, ts(7), 1),
            ("orphan".to_string(), 5, 5, 5, ts(6), 2),
            ("neg".to_string(), 0, 0, 0, ts(5), 1),
            ("neg".to_string(), -1, -2, 7, ts(4), 2),
        ];
        let folded = super::fold_last_turns(rows);
        assert_eq!(
            folded["a"],
            super::LastTurn { cache_read: 200, cache_creation: 300, at: ts(9), prev_context: 60 }
        );
        assert_eq!(
            folded["b"],
            super::LastTurn { cache_read: 2, cache_creation: 3, at: ts(7), prev_context: 0 }
        );
        assert!(!folded.contains_key("orphan"));
        assert_eq!(folded["neg"].prev_context, 7);
    }

    #[test]
    fn enrich_with_an_empty_context_leaves_items_untouched() {
        let mut item = bare_session("s");
        item.machine_name = Some("kept".into());
        let before = serde_json::to_value(&item).unwrap();
        let out = super::enrich(vec![(ts(0), item)], super::EnrichContext::default());
        assert_eq!(out.len(), 1);
        assert_eq!(serde_json::to_value(&out[0].1).unwrap(), before);
    }

    #[test]
    fn enrich_merges_every_signal_by_session_id() {
        let mut item = bare_session("s");
        item.machine_id = "host-a".into();
        item.last_heartbeat = Some(ts(0));
        let mut other = bare_session("other");
        other.machine_id = "unknown".into();

        let mut ctx = super::EnrichContext { archive_after_secs: 60, ..Default::default() };
        ctx.machines.insert("host-a".into(), ("Alpha".into(), Some(7), "vm".into()));
        ctx.token_usage.insert(
            "s".into(),
            cctui_proto::models::TokenUsage { tokens_in: 11, tokens_out: 3, ..Default::default() },
        );
        ctx.last_messages.insert("s".into(), (Some("hello".into()), ts(5)));
        ctx.unread = Some(std::iter::once(("s".to_string(), 4)).collect());
        ctx.todos
            .insert("s".into(), serde_json::json!([{"content": "do it", "status": "pending"}]));
        ctx.signals.insert(
            "s".into(),
            super::SignalRow {
                id: "s".into(),
                tempo: Some("hibernated".into()),
                activity: Some("editing".into()),
                session_name: Some("named".into()),
                model: Some("m".into()),
                effort: Some("high".into()),
                soft_limit_reason: None,
                last_tool_name: Some("Bash".into()),
                tool_use_count: -3,
                children: serde_json::json!([
                    {"id": "1", "href": "https://x/pr/1", "kind": "pr"},
                    {"id": "2", "href": "https://x/issue/2", "kind": "issue"}
                ]),
                end_reason: Some("completed".into()),
                end_detail: Some("done".into()),
                ended_at: Some(ts(9)),
                permission_mode: Some("yolo".into()),
                archived_by: Some("user".into()),
                keepalive_json: Some(serde_json::json!({"interval_secs": 5, "max_ticks": 2})),
                last_keepalive_at: Some(ts(8)),
                ..Default::default()
            },
        );
        ctx.labels.insert(
            "s".into(),
            vec![super::Label { id: "l1".into(), name: "bug".into(), color: "#f00".into() }],
        );
        ctx.account_names.insert("s".into(), "acct".into());
        ctx.with_credentials.insert("s".into());
        ctx.traffic_observed.insert("s".into());
        ctx.auto_approve.insert("s".into());
        ctx.last_turns.insert(
            "s".into(),
            super::LastTurn { cache_read: 100, cache_creation: 900, at: ts(7), prev_context: 1000 },
        );

        let out = super::enrich(vec![(ts(0), item), (ts(1), other)], ctx);
        let s = &out[0].1;
        assert_eq!(s.machine_name.as_deref(), Some("Alpha"));
        assert_eq!(s.machine_hue, Some(7));
        assert_eq!(s.machine_kind.as_deref(), Some("vm"));
        assert_eq!((s.token_usage.tokens_in, s.token_usage.tokens_out), (11, 3));
        assert_eq!(s.last_message_text.as_deref(), Some("hello"));
        assert_eq!(s.last_message_at, Some(ts(5)));
        assert_eq!(s.unread_count, 4);
        assert_eq!(s.todos.len(), 1);
        assert_eq!(s.todos[0].content, "do it");
        assert!(s.hibernated);
        assert_eq!(s.name.as_deref(), Some("named"));
        assert_eq!(s.model.as_deref(), Some("m"));
        assert_eq!(s.effort.as_deref(), Some("high"));
        assert_eq!(s.permission_mode.as_deref(), Some("yolo"));
        assert!(!s.pinned);
        assert_eq!(s.activity_detail.as_deref(), Some("editing"));
        assert_eq!(s.last_tool_name.as_deref(), Some("Bash"));
        assert_eq!(s.tool_use_count, 0);
        assert_eq!(s.pr_links, ["https://x/pr/1"]);
        assert_eq!(s.end_reason, Some(cctui_proto::models::SessionEndReason::Completed));
        assert_eq!(s.end_detail.as_deref(), Some("done"));
        assert_eq!(s.ended_at, Some(ts(9)));
        assert_eq!(s.archived_by, Some(RemoveInitiator::User));
        assert_eq!(s.auto_archive_at, Some(ts(60)));
        assert_eq!(s.keepalive.as_ref().map(|k| k.max_ticks), Some(2));
        assert_eq!(s.last_keepalive_at, Some(ts(8)));
        assert_eq!(s.labels.len(), 1);
        assert_eq!(s.labels[0].name, "bug");
        assert_eq!(s.account_name.as_deref(), Some("acct"));
        assert!(s.has_token_credentials);
        assert!(s.account_traffic_observed);
        assert!(s.auto_approve);
        assert_eq!(s.last_activity_at, Some(ts(7)));
        assert!(s.cache_cold);
        assert_eq!(s.estimated_burst_tokens, Some(1000));

        let o = &out[1].1;
        assert_eq!(o.machine_name, None);
        assert_eq!(o.unread_count, 0);
        assert!(!o.has_token_credentials && !o.account_traffic_observed && !o.auto_approve);
        assert_eq!(o.account_name, None);
        assert!(o.labels.is_empty());
    }

    #[test]
    fn enrich_keeps_a_machine_name_the_row_already_resolved() {
        let mut item = bare_session("s");
        item.machine_id = "host-a".into();
        item.machine_name = Some("from-join".into());
        let mut ctx = super::EnrichContext::default();
        ctx.machines.insert("host-a".into(), ("Alpha".into(), Some(7), "vm".into()));
        let out = super::enrich(vec![(ts(0), item)], ctx);
        assert_eq!(out[0].1.machine_name.as_deref(), Some("from-join"));
        assert_eq!(out[0].1.machine_hue, None);
    }

    #[test]
    fn enrich_without_a_viewer_leaves_unread_at_zero() {
        let item = bare_session("s");
        let ctx = super::EnrichContext { unread: None, ..Default::default() };
        let out = super::enrich(vec![(ts(0), item)], ctx);
        assert_eq!(out[0].1.unread_count, 0);
    }

    #[test]
    fn fold_token_totals_sums_models_and_reads_null_as_zero() {
        let rows = vec![
            ("s".to_string(), Some("m1".to_string()), Some(10), Some(2), Some(5), Some(1)),
            ("s".to_string(), None, Some(1), None, None, Some(-4)),
            ("t".to_string(), Some("m1".to_string()), None, None, None, None),
        ];
        let folded = super::fold_token_totals(rows, &std::collections::HashMap::new());
        let s = &folded["s"];
        assert_eq!(
            (s.tokens_in, s.tokens_out, s.cache_read_tokens, s.cache_creation_tokens),
            (11, 2, 5, 1)
        );
        assert!(s.cost_usd.abs() < f64::EPSILON);
        let t = &folded["t"];
        assert_eq!((t.tokens_in, t.tokens_out), (0, 0));
    }

    #[test]
    fn cap_unread_clamps_to_badge_ceiling() {
        assert_eq!(cap_unread(0), 0);
        assert_eq!(cap_unread(5), 5);
        assert_eq!(cap_unread(99), 99);
        assert_eq!(cap_unread(100), 99);
        assert_eq!(cap_unread(1_000_000), 99);
        assert_eq!(cap_unread(-1), 0);
    }

    #[test]
    fn compile_plain_keyword_matches_legacy_shape() {
        let (sql, params) = compile("hello");
        assert!(sql.contains("s.id ILIKE $1"));
        assert!(sql.contains("s.working_dir ILIKE $1"));
        assert!(sql.contains("c.body ILIKE $1"));
        assert_eq!(params, vec!["%hello%".to_string()]);
    }

    #[test]
    fn free_text_patterns_are_folded_like_the_chunks() {
        let (_, params) = compile("\"it\u{2019}s  \u{201C}done\u{201D}\"");
        assert_eq!(params, vec!["%it's \"done\"%".to_string()]);
    }

    #[test]
    fn compile_multi_keywords_and() {
        let (sql, params) = compile("foo bar");
        assert!(sql.contains(" AND "));
        assert_eq!(params, vec!["%foo%".to_string(), "%bar%".to_string()]);
        assert!(sql.contains("$1") && sql.contains("$2"));
    }

    #[test]
    fn compile_or_between_keywords() {
        let (sql, _) = compile("foo OR bar");
        // Terms joined by OR at the top level; the only AND is the one inside
        // the free-text EXISTS subquery, never a top-level `) AND (` join.
        assert!(sql.contains(") OR ("), "top-level OR join:\n{sql}");
        assert!(!sql.contains(") AND ("), "no top-level AND join:\n{sql}");
    }

    #[test]
    fn compile_machine_filter() {
        let (sql, params) = compile("machine:dev1");
        assert!(sql.contains("COALESCE(m.display_name, m.name)) = lower($1)"));
        assert!(sql.contains("lower(s.machine_id) = lower($1)"));
        assert_eq!(params, vec!["dev1".to_string()]);
    }

    #[test]
    fn compile_tag_filter_uses_label_join() {
        let (sql, params) = compile("tag:infra");
        assert!(sql.contains("session_labels"));
        assert!(sql.contains("lower(l.name) = lower($1)"));
        assert_eq!(params, vec!["infra".to_string()]);
    }

    #[test]
    fn compile_account_filter_uses_token_join() {
        let (sql, _) = compile("account:personal");
        assert!(sql.contains("session_tokens"));
        assert!(sql.contains("lower(a.name) = lower($1)"));
    }

    #[test]
    fn compile_title_contains() {
        let (sql, params) = compile("title:fix");
        assert!(sql.contains("s.session_name, '') ILIKE $1"));
        assert_eq!(params, vec!["%fix%".to_string()]);
    }

    #[test]
    fn compile_model_filter_falls_back_to_metadata() {
        let (sql, params) = compile("model:claude-opus-4-7[1m]");
        assert!(sql.contains("COALESCE(s.model, s.metadata->>'model', '') ILIKE $1"));
        assert_eq!(params, vec!["%claude-opus-4-7[1m]%".to_string()]);
    }

    #[test]
    fn compile_negation() {
        let (sql, _) = compile("-machine:dev1");
        assert!(sql.starts_with("(NOT "));
    }

    #[test]
    fn compile_group_or_and_keyword() {
        let (sql, params) = compile("( machine:m2pro OR machine:dev1 ) AND keyword");
        assert!(sql.contains(" OR "));
        assert!(sql.contains(" AND "));
        assert_eq!(params.len(), 3);
        assert_eq!(params[2], "%keyword%");
    }

    #[test]
    fn compile_pinned_binds_bool() {
        let mut params = Vec::new();
        let sql = compile_node(&cctui_query::parse("pinned:true"), &mut params);
        assert!(sql.contains("s.pinned = $1"));
        assert!(matches!(params.as_slice(), [SqlParam::Bool(true)]));
    }

    #[test]
    fn compile_status_in_list() {
        let (sql, params) = compile("status:active,inactive");
        assert!(sql.contains("lower(s.status) = lower($1)"));
        assert!(sql.contains("lower(s.status) = lower($2)"));
        assert!(sql.contains(" OR "));
        assert_eq!(params, vec!["active".to_string(), "inactive".to_string()]);
    }

    #[test]
    fn compile_malformed_degrades_to_text_not_error() {
        let (sql, params) = compile("( machine:dev1 AND");
        assert!(sql.contains("lower(s.machine_id) = lower($1)"));
        assert_eq!(params, vec!["dev1".to_string()]);
        let (sql, _) = compile("unknown:field:mess ((");
        assert!(sql.contains("ILIKE $1"));
    }

    #[test]
    fn compile_empty_is_true() {
        let (sql, params) = compile("");
        assert_eq!(sql, "TRUE");
        assert!(params.is_empty());
    }

    #[test]
    fn compile_dir_and_effort() {
        let (sql, params) = compile("dir:cctui effort:high");
        assert!(sql.contains("s.working_dir ILIKE $1"));
        assert!(sql.contains("lower(COALESCE(s.effort, '')) = lower($2)"));
        assert_eq!(params, vec!["%cctui%".to_string(), "high".to_string()]);
    }

    fn attention_from_signals(
        tempo: Option<&str>,
        agent_state: Option<&str>,
        activity: Option<&str>,
    ) -> Option<Attention> {
        attention_from_bucket(bucket_from_signals(
            tempo,
            agent_state,
            activity,
            None,
            &[],
            &std::collections::HashMap::new(),
        ))
    }

    #[test]
    fn liveness_active_within_active_window() {
        let hb = Utc::now() - Duration::seconds(60);
        assert_eq!(derive_liveness(hb), Liveness::Active);
    }

    #[test]
    fn liveness_stale_between_windows() {
        let hb = Utc::now() - Duration::minutes(20);
        assert_eq!(derive_liveness(hb), Liveness::Stale);
    }

    #[test]
    fn liveness_dead_past_dead_window() {
        let hb = Utc::now() - Duration::hours(3);
        assert_eq!(derive_liveness(hb), Liveness::Dead);
    }

    #[test]
    fn attention_needs_input_when_tempo_blocked() {
        assert_eq!(
            attention_from_signals(Some("blocked"), Some("working"), None),
            Some(Attention::NeedsInput),
        );
    }

    #[test]
    fn no_attention_when_tempo_active() {
        assert_eq!(attention_from_signals(Some("active"), Some("working"), None), None);
    }

    #[test]
    fn no_attention_when_blocked_but_already_failed() {
        // activity=failure short-circuits to Done before the blocked check.
        assert_eq!(attention_from_signals(Some("blocked"), None, Some("failure")), None);
    }

    #[test]
    fn no_attention_when_no_signals() {
        assert_eq!(attention_from_signals(None, None, None), None);
    }

    fn bucket4(
        tempo: Option<&str>,
        agent_state: Option<&str>,
        activity: Option<&str>,
        soft_limit: Option<&str>,
    ) -> Bucket {
        bucket_from_signals(
            tempo,
            agent_state,
            activity,
            soft_limit,
            &[],
            &std::collections::HashMap::new(),
        )
    }

    #[test]
    fn bucket_reflects_signals() {
        use cctui_proto::classifier::Bucket;
        assert_eq!(bucket4(Some("blocked"), Some("working"), None, None), Bucket::Blocked);
        assert_eq!(bucket4(Some("active"), None, None, None), Bucket::Working);
        assert_eq!(bucket4(None, Some("stopped"), None, None), Bucket::Done);
        assert_eq!(bucket4(None, None, Some("success"), None), Bucket::Done);
        // a durable soft-limit block forces Blocked even over an
        // otherwise-Done/active session.
        assert_eq!(
            bucket4(Some("active"), None, Some("success"), Some("rate-limited")),
            Bucket::Blocked
        );
    }

    #[test]
    fn pr_children_open_review_lands_in_review_bucket() {
        use cctui_proto::classifier::{OwnedPrStatus, PrStatusCache};
        let children = [SessionChild {
            id: "1".into(),
            href: "https://github.com/o/r/pull/1".into(),
            kind: "pr".into(),
        }];
        let cache = PrStatusCache::new();
        cache.upsert(
            "https://github.com/o/r/pull/1",
            OwnedPrStatus {
                state: "OPEN".into(),
                checks_passed: 3,
                checks_failed: 1,
                checks_pending: 0,
                review: "REVIEW_REQUIRED".into(),
            },
        );
        let snap = cache.snapshot();
        let map = PrStatusCache::borrow_map(&snap);
        assert_eq!(
            bucket_from_signals(None, Some("working"), None, None, &children, &map),
            Bucket::Review
        );
        cache.remove("https://github.com/o/r/pull/1");
        let snap2 = cache.snapshot();
        let map2 = PrStatusCache::borrow_map(&snap2);
        assert_eq!(
            bucket_from_signals(None, Some("working"), None, None, &children, &map2),
            Bucket::Working
        );
    }

    #[test]
    fn collapses_newlines_and_runs_of_whitespace() {
        let raw = "line one\n\nline two\t\t  line three";
        assert_eq!(normalize_last_message(raw), "line one line two line three");
    }

    #[test]
    fn truncates_with_ellipsis_past_max() {
        let raw = "a".repeat(250);
        let out = normalize_last_message(&raw);
        // 200 chars + the ellipsis.
        assert_eq!(out.chars().count(), 201);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn does_not_ellipsis_short_text() {
        let raw = "short message";
        assert_eq!(normalize_last_message(raw), "short message");
    }

    #[test]
    fn handles_cjk_and_emoji_by_char_count_not_bytes() {
        // 200 of these = 200 chars, well under the cap.
        let raw = "漢".repeat(200);
        let out = normalize_last_message(&raw);
        assert_eq!(out.chars().count(), 200);
        assert!(!out.ends_with('…'));
    }

    #[test]
    fn truncates_cjk_at_char_boundary_not_byte() {
        // 250 CJK chars (3 bytes each in UTF-8 = 750 bytes).
        let raw = "漢".repeat(250);
        let out = normalize_last_message(&raw);
        assert_eq!(out.chars().count(), 201);
        assert!(out.ends_with('…'));
    }

    /// Reference `make_snippet` oracle: the optimised version must produce
    /// byte-identical snippets.
    fn make_snippet_reference(text: &str, needles: &[String]) -> String {
        const WINDOW: usize = 200;
        let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
        let hay = collapsed.to_lowercase();
        let chars: Vec<char> = collapsed.chars().collect();
        let match_byte =
            needles.iter().filter_map(|n| hay.find(&n.to_lowercase())).min().unwrap_or(0);
        let match_char = collapsed[..match_byte].chars().count();
        let start = match_char.saturating_sub(WINDOW / 2);
        let end = (start + WINDOW).min(chars.len());
        let mut out = String::new();
        if start > 0 {
            out.push('\u{2026}');
        }
        out.extend(&chars[start..end]);
        if end < chars.len() {
            out.push('\u{2026}');
        }
        out
    }

    #[test]
    fn snippet_output_is_byte_identical_to_the_reference() {
        let long = "lorem ipsum dolor sit amet ".repeat(60);
        let unicode =
            "\u{3053}\u{3093}\u{306b}\u{3061}\u{306f} caf\u{e9} na\u{ef}ve \u{1f980} crab "
                .repeat(40);
        let cases: Vec<(&str, Vec<String>)> = vec![
            ("", vec!["x".into()]),
            ("short text", vec![]),
            ("short text", vec!["nomatch".into()]),
            ("the quick brown fox", vec!["brown".into()]),
            ("The Quick BROWN Fox", vec!["brown".into()]),
            ("  collapse   all \n\t whitespace  runs ", vec!["all".into()]),
            (long.as_str(), vec!["dolor".into()]),
            (long.as_str(), vec!["nomatch".into()]),
            (unicode.as_str(), vec!["crab".into()]),
            (unicode.as_str(), vec!["caf\u{e9}".into()]),
            (unicode.as_str(), vec!["\u{1f980}".into()]),
            // Multi-needle: the window centres on the earliest hit, not the first listed.
            (long.as_str(), vec!["amet".into(), "lorem".into()]),
            ("exactly at the boundary", vec!["boundary".into()]),
        ];
        for (text, needles) in cases {
            assert_eq!(
                make_snippet(text, &needles),
                make_snippet_reference(text, &needles),
                "snippet drifted for {text:?} / {needles:?}"
            );
        }
    }

    #[test]
    fn snippet_never_exceeds_the_window() {
        let long = "abcdefghij ".repeat(200);
        let out = make_snippet(&long, &["abcdefghij".to_string()]);
        // 200 chars plus at most one ellipsis on each side.
        assert!(out.chars().count() <= 202, "{}", out.chars().count());
    }

    #[test]
    fn snippet_sql_returns_one_row_per_session() {
        let sql = snippet_sql(1);
        assert!(sql.contains("CROSS JOIN LATERAL"), "{sql}");
        assert!(sql.contains("ORDER BY c.event_id DESC LIMIT 50"), "ranks a bounded set: {sql}");
        assert!(sql.contains("plainto_tsquery('simple', $3)"), "{sql}");
        assert!(sql.contains("unnest($1::text[])"), "{sql}");
        // A per-session LATERAL replaces the whole-table DISTINCT ON: bringing
        // that back re-scans every matching event of every session.
        assert!(!sql.contains("DISTINCT ON"), "{sql}");
    }

    #[test]
    fn snippet_sql_binds_every_pattern_then_the_rank_terms() {
        let sql = snippet_sql(3);
        for i in 2..=4 {
            assert!(sql.contains(&format!("c.body ILIKE ${i}")), "{sql}");
        }
        assert!(sql.contains("plainto_tsquery('simple', $5)"), "{sql}");
        assert!(!sql.contains("$6"), "{sql}");
    }

    #[test]
    fn free_text_exists_keeps_the_correlation_barrier() {
        let (sql, _) = compile("hello");
        // `(SELECT s.id)` is what stops the planner hoisting the sublink into a
        // hashed subplan that rechecks every matching chunk in the table.
        assert!(sql.contains("c.session_id = (SELECT s.id)"), "{sql}");
    }

    /// The persisted posture reaches the sessions payload and the
    /// wire shape only carries the key when it is known.
    #[test]
    fn permission_mode_rides_the_session_payload() {
        let mut item = bare_session("perm-1");
        assert!(item.permission_mode.is_none());
        let bare = serde_json::to_value(&item).unwrap();
        assert!(bare.get("permission_mode").is_none(), "an unknown posture must not be serialized");

        item.permission_mode = Some("yolo".into());
        let wire = serde_json::to_value(&item).unwrap();
        assert_eq!(wire["permission_mode"], serde_json::json!("yolo"));

        let back: cctui_proto::api::SessionListItem = serde_json::from_value(wire).unwrap();
        assert_eq!(back.permission_mode.as_deref(), Some("yolo"));
    }

    /// The get-one fallback selects the column, so an archived session still
    /// reports the posture it ran under.
    #[tokio::test]
    async fn get_session_selects_the_persisted_permission_mode() {
        let Some((pool, sid)) = seeded_session("get_session_permission_mode").await else {
            return;
        };
        sqlx::query("UPDATE sessions SET permission_mode = 'plan' WHERE id = $1")
            .bind(&sid)
            .execute(&pool)
            .await
            .expect("set posture");

        let row: Option<EndRow> = sqlx::query_as(
            "SELECT end_reason, end_detail, ended_at, todos, permission_mode, pinned, archived_by \
             FROM sessions WHERE id = $1",
        )
        .bind(&sid)
        .fetch_optional(&pool)
        .await
        .expect("read session");
        assert_eq!(row.expect("session row").4.as_deref(), Some("plan"));

        sqlx::query("DELETE FROM sessions WHERE id = $1")
            .bind(&sid)
            .execute(&pool)
            .await
            .expect("cleanup");
    }

    /// Opening a session that is no longer live reads it through
    /// `fetch_by_id`; its SELECT once lacked `room_id`, so every archived open
    /// failed with `no column found for name: room_id`.
    #[tokio::test]
    async fn an_archived_session_opens_from_the_db_with_its_room() {
        let Some((pool, sid)) = seeded_session("archived_session_opens_with_room").await else {
            return;
        };
        let room: uuid::Uuid = sqlx::query_scalar(
            "INSERT INTO rooms (user_id, name) \
             SELECT user_id, 'wave 23' FROM sessions WHERE id = $1 RETURNING id",
        )
        .bind(&sid)
        .fetch_one(&pool)
        .await
        .expect("seed room");
        sqlx::query("UPDATE sessions SET room_id = $1, status = 'archived' WHERE id = $2")
            .bind(room)
            .bind(&sid)
            .execute(&pool)
            .await
            .expect("archive into room");

        let row = crate::store::sessions::fetch_by_id(&pool, &sid)
            .await
            .expect("fetch_by_id must decode a full DbSession")
            .expect("session row");
        assert_eq!(row.status, "archived");
        assert_eq!(row.room_id, Some(room));
        assert_eq!(row.room_name.as_deref(), Some("wave 23"));

        sqlx::query("DELETE FROM sessions WHERE id = $1")
            .bind(&sid)
            .execute(&pool)
            .await
            .expect("cleanup");
    }

    #[test]
    fn the_remove_command_carries_who_asked() {
        let id = uuid::Uuid::new_v4();
        for initiator in [RemoveInitiator::User, RemoveInitiator::Automatic] {
            let cctui_proto::adapter::AdapterCommand::Remove {
                local_id,
                command_id,
                initiator: got,
            } = remove_command("sess-1", id, initiator)
            else {
                panic!("remove_command must build a Remove")
            };
            assert_eq!(local_id, "sess-1");
            assert_eq!(command_id, Some(id));
            assert_eq!(got, initiator);
        }
    }

    #[tokio::test]
    async fn archiving_forgets_the_guard_step_mirror() {
        let Some(url) = crate::routes::gateway::test_db_url("archiving_forgets_the_guard_step")
        else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
        let state = crate::state::AppState::for_test(pool.clone());
        let (uid, machine) = crate::events::tests::seed_user_machine(&pool, "arch-guard").await;
        let sid = crate::events::tests::seed_session(&pool, uid, machine, Some("g")).await;
        crate::guard_step::record(
            sid.clone(),
            cctui_proto::guard_step::GuardStepState { step: 2, ..Default::default() },
        );
        let outcome = super::archive_one(
            &state,
            &sid,
            true,
            RemoveInitiator::User,
            crate::events::Actor::User(uid),
        )
        .await
        .unwrap();
        assert_eq!(outcome, super::ArchiveOutcome::Archived);
        assert_eq!(crate::guard_step::get(&sid), None);
    }

    #[test]
    fn missing_parent_ids_lists_each_absent_nesting_parent_once() {
        let mut parent = bare_session("p");
        let mut a = bare_session("a");
        a.parent_id = Some("live-orch".into());
        let mut b = bare_session("b");
        b.parent_id = Some("live-orch".into());
        let mut nested = bare_session("n");
        nested.parent_id = Some("p".into());
        let mut fork = bare_session("f");
        fork.parent_id = Some("elsewhere".into());
        fork.metadata = serde_json::json!({"relation": "fork"});
        parent.parent_id = Some("other-page".into());
        let page = [a, parent, b, nested, fork, bare_session("root")];
        assert_eq!(
            super::missing_parent_ids(&page),
            vec!["live-orch".to_owned(), "other-page".to_owned()]
        );
    }

    #[tokio::test]
    async fn parent_stubs_carry_status_and_stay_owner_scoped() {
        let Some((pool, sid)) = seeded_session("parent_stubs_owner_scoped").await else {
            return;
        };
        let owner: uuid::Uuid = sqlx::query_scalar(
            "SELECT m.user_id FROM sessions s JOIN machines m ON m.id = s.machine_uuid \
             WHERE s.id = $1",
        )
        .bind(&sid)
        .fetch_one(&pool)
        .await
        .expect("owner");
        sqlx::query("UPDATE sessions SET session_name = 'orch-x' WHERE id = $1")
            .bind(&sid)
            .execute(&pool)
            .await
            .expect("name parent");
        let ids = [sid.clone(), "no-such-session".to_owned()];

        let live = super::fetch_parent_stubs(&pool, &ids, Some(owner)).await.expect("stubs");
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].id, sid);
        assert_eq!(live[0].name.as_deref(), Some("orch-x"));
        assert_eq!(live[0].working_dir, "/w");
        assert_ne!(live[0].status, cctui_proto::models::SessionStatus::Archived);

        let stranger = uuid::Uuid::new_v4();
        assert!(super::fetch_parent_stubs(&pool, &ids, Some(stranger)).await.unwrap().is_empty());
        assert!(super::fetch_parent_stubs(&pool, &[], None).await.unwrap().is_empty());

        sqlx::query("UPDATE sessions SET status = 'archived' WHERE id = $1")
            .bind(&sid)
            .execute(&pool)
            .await
            .expect("archive parent");
        let archived = super::fetch_parent_stubs(&pool, &ids, None).await.expect("admin stubs");
        assert_eq!(archived[0].status, cctui_proto::models::SessionStatus::Archived);

        sqlx::query("DELETE FROM sessions WHERE id = $1")
            .bind(&sid)
            .execute(&pool)
            .await
            .expect("cleanup");
    }
}
