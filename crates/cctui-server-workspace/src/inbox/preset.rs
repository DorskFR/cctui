use axum::Json;
use cctui_proto::api::presets::OnStale;
use cctui_proto::api::{DispatchRequest, SpawnRequest};
use sqlx::PgPool;
use uuid::Uuid;

use super::claim::{BatchEvent, ClaimedLane};
use super::deliver::Outcome;
use crate::state::AppState;

pub const REGISTER_WAIT_SECS: i64 = 300;
pub const PRESET_GONE: &str = "the lane's preset no longer exists";
pub const MACHINE_GONE: &str = "the preset's machine no longer exists or was revoked";
pub const DISPATCHER_GONE: &str = "the preset's dispatcher no longer exists or was revoked";
pub const UNDELIVERED: &str = "an earlier attempt of this batch never reached the daemon";
const LABELLED_SOURCES: &[&str] = &["schedule", "webhook"];
const RUN_LABEL_COLOR: &str = "210";
const MAX_BATCH_FILE_BYTES: usize = 1024 * 1024;
const NOT_REUSABLE: &[&str] = &["ended", "archived", "failed", "queued", "draft"];

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Preset {
    pub user_id: Uuid,
    pub name: String,
    pub prompt: String,
    pub machine_id: Option<Uuid>,
    pub working_dir: Option<String>,
    pub adapter_id: Option<String>,
    pub model: Option<String>,
    pub permission_mode: Option<String>,
    pub profile: Option<String>,
    pub fresh_secs: i32,
    pub on_stale: String,
    pub notify_url: Option<String>,
    pub machine_live: bool,
    pub dispatcher_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Current {
    pub id: String,
    pub status: String,
    pub adapter_id: Option<String>,
    pub model: Option<String>,
    pub idle_secs: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    Reply(String),
    Followup { parent: String },
    Spawn,
}

impl Plan {
    #[must_use]
    pub const fn delivered_as(&self) -> &'static str {
        match self {
            Self::Reply(_) => "reply",
            Self::Followup { .. } => "followup",
            Self::Spawn => "spawn",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prior {
    None,
    Launched(String),
    Undelivered,
    Refused(String),
}

#[must_use]
pub fn reuse_window_secs(fresh_secs: i32, current: &Current) -> i64 {
    let cache = crate::keepalive::default_interval_secs(
        current.adapter_id.as_deref(),
        current.model.as_deref(),
    );
    i64::from(fresh_secs).min(i64::from(cache))
}

#[must_use]
pub fn plan(
    fresh_secs: i32,
    on_stale: OnStale,
    current: Option<&Current>,
    parent_override: Option<&str>,
) -> Plan {
    if let Some(s) = current
        && !NOT_REUSABLE.contains(&s.status.as_str())
        && s.idle_secs < reuse_window_secs(fresh_secs, s)
    {
        return Plan::Reply(s.id.clone());
    }
    let parent = parent_override.or_else(|| current.map(|s| s.id.as_str()));
    match (on_stale, parent) {
        (OnStale::Followup, Some(parent)) => Plan::Followup { parent: parent.to_owned() },
        _ => Plan::Spawn,
    }
}

/// A reply carries text only, so a batch with files starts a session instead.
#[must_use]
pub fn with_files(plan: Plan, on_stale: OnStale, has_files: bool) -> Plan {
    match (plan, on_stale) {
        (Plan::Reply(parent), OnStale::Followup) if has_files => Plan::Followup { parent },
        (Plan::Reply(_), OnStale::New) if has_files => Plan::Spawn,
        (plan, _) => plan,
    }
}

#[must_use]
pub fn prior_from(state: &str, detail: Option<String>, session_id: String, alive: bool) -> Prior {
    if alive {
        return Prior::Launched(session_id);
    }
    match state {
        "refused" => Prior::Refused(detail.unwrap_or_else(|| "no detail".to_owned())),
        "delivery_failed" => Prior::Undelivered,
        _ => Prior::Launched(session_id),
    }
}

pub async fn prior_launch(pool: &PgPool, batch_id: Uuid) -> sqlx::Result<Prior> {
    let row: Option<(String, Option<String>, String, bool)> = sqlx::query_as(
        "SELECT ml.state, ml.detail, COALESCE(ml.registered_session_id, ml.session_id), \
                EXISTS (SELECT 1 FROM sessions s \
                        WHERE s.id = COALESCE(ml.registered_session_id, ml.session_id)) \
         FROM machine_launches ml WHERE ml.command_id = $1",
    )
    .bind(batch_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map_or(Prior::None, |(state, detail, session, alive)| {
        prior_from(&state, detail, session, alive)
    }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::FromRow)]
pub struct Found {
    pub id: Uuid,
    pub user_id: Uuid,
    pub external_sources: bool,
}

/// `owner` is [`crate::auth::AuthContext::owner_filter`]; an id beats a name.
pub async fn find(pool: &PgPool, owner: Option<Uuid>, preset: &str) -> sqlx::Result<Option<Found>> {
    sqlx::query_as(
        "SELECT id, user_id, external_sources FROM presets \
         WHERE (id::text = $1 OR name = $1) AND ($2::uuid IS NULL OR user_id = $2) \
         ORDER BY id::text = $1 DESC, created_at LIMIT 1",
    )
    .bind(preset)
    .bind(owner)
    .fetch_optional(pool)
    .await
}

pub async fn lane(
    pool: &PgPool,
    user_id: Uuid,
    preset_id: Uuid,
    topic: &str,
) -> sqlx::Result<Uuid> {
    sqlx::query_scalar(
        "INSERT INTO inbox_lanes (user_id, key, preset_id) VALUES ($1, $2, $3) \
         ON CONFLICT (user_id, key) DO UPDATE SET preset_id = EXCLUDED.preset_id \
         RETURNING id",
    )
    .bind(user_id)
    .bind(super::preset_lane_key(preset_id, topic))
    .bind(preset_id)
    .fetch_one(pool)
    .await
}

pub async fn from_session(
    pool: &PgPool,
    owner: Uuid,
    session_id: &str,
) -> sqlx::Result<Option<Preset>> {
    let found: Option<Preset> = sqlx::query_as(
        "SELECT $2::uuid AS user_id, COALESCE(s.session_name, '') AS name, '' AS prompt, \
                s.machine_uuid AS machine_id, s.working_dir, s.adapter_id, s.model, \
                s.permission_mode, NULL::text AS profile, 0 AS fresh_secs, \
                'followup' AS on_stale, NULL::text AS notify_url, \
                m.id IS NOT NULL AS machine_live, NULL::text AS dispatcher_name \
         FROM sessions s \
         LEFT JOIN machines m ON m.id = s.machine_uuid \
              AND m.revoked_at IS NULL AND m.deleted_at IS NULL \
         WHERE s.id = $1",
    )
    .bind(session_id)
    .bind(owner)
    .fetch_optional(pool)
    .await?;
    Ok(found.map(|mut target| {
        target.permission_mode = target
            .permission_mode
            .as_deref()
            .and_then(cctui_proto::adapter::PermissionMode::from_session_label)
            .and_then(|mode| serde_json::to_value(mode).ok())
            .and_then(|v| v.as_str().map(ToOwned::to_owned));
        target
    }))
}

/// A session lane whose session is gone follows up from the parent its events
/// name; `None` when they name none, or it no longer exists.
pub async fn follow_up_orphan(
    state: &AppState,
    lane: &ClaimedLane,
    events: &[BatchEvent],
) -> sqlx::Result<Option<Outcome>> {
    let Some(parent) = parent_override(events) else { return Ok(None) };
    let Some(target) = from_session(&state.pool, lane.user_id, parent).await? else {
        return Ok(None);
    };
    let plan = Plan::Followup { parent: parent.to_owned() };
    spawn(state, lane, &target, &plan, events, Vec::new()).await.map(Some)
}

pub async fn load(pool: &PgPool, preset_id: Uuid) -> sqlx::Result<Option<Preset>> {
    sqlx::query_as(
        "SELECT p.user_id, p.name, p.prompt, p.machine_id, p.working_dir, p.adapter_id, \
                p.model, p.permission_mode, p.profile, p.fresh_secs, p.on_stale, p.notify_url, \
                m.id IS NOT NULL AS machine_live, d.name AS dispatcher_name \
         FROM presets p \
         LEFT JOIN machines m ON m.id = p.machine_id AND m.user_id = p.user_id \
              AND m.revoked_at IS NULL AND m.deleted_at IS NULL \
         LEFT JOIN dispatchers d ON d.id = p.dispatcher_id AND d.user_id = p.user_id \
              AND d.revoked_at IS NULL AND d.deleted_at IS NULL \
         WHERE p.id = $1",
    )
    .bind(preset_id)
    .fetch_optional(pool)
    .await
}

async fn current(pool: &PgPool, session_id: &str) -> sqlx::Result<Option<Current>> {
    sqlx::query_as(
        "SELECT id, status, adapter_id, model, \
                EXTRACT(EPOCH FROM now() - last_heartbeat)::bigint AS idle_secs \
         FROM sessions WHERE id = $1",
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    Session(Option<Current>),
    /// A just-registered spawn may be mid-turn: only the claim filter knows.
    Wait,
}

pub async fn resolve(pool: &PgPool, lane: &ClaimedLane) -> sqlx::Result<Resolved> {
    if let Some(session_id) = lane.session_id.as_deref() {
        return Ok(Resolved::Session(current(pool, session_id).await?));
    }
    let Some(command) = lane.spawn_command_id else { return Ok(Resolved::Session(None)) };
    let launch: Option<(String, Option<String>, bool)> = sqlx::query_as(
        "SELECT ml.state, s.id, ml.created_at > now() - $2::bigint * interval '1 second' \
         FROM machine_launches ml \
         LEFT JOIN sessions s ON s.id = COALESCE(ml.registered_session_id, ml.session_id) \
         WHERE ml.command_id = $1",
    )
    .bind(command)
    .bind(REGISTER_WAIT_SECS)
    .fetch_optional(pool)
    .await?;
    match launch {
        Some((_, Some(session_id), _)) => {
            sqlx::query(
                "UPDATE inbox_lanes SET session_id = $2, spawn_command_id = NULL \
                 WHERE id = $1 AND spawn_command_id = $3",
            )
            .bind(lane.id)
            .bind(&session_id)
            .bind(command)
            .execute(pool)
            .await?;
            Ok(Resolved::Wait)
        }
        Some((state, None, true)) if !matches!(state.as_str(), "refused" | "delivery_failed") => {
            Ok(Resolved::Wait)
        }
        _ => {
            set_spawn(pool, lane.id, None).await?;
            Ok(Resolved::Session(None))
        }
    }
}

async fn set_spawn(pool: &PgPool, lane_id: Uuid, command: Option<Uuid>) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE inbox_lanes SET spawn_command_id = $2, \
             session_id = CASE WHEN $2::uuid IS NULL THEN session_id END \
         WHERE id = $1",
    )
    .bind(lane_id)
    .bind(command)
    .execute(pool)
    .await
    .map(|_| ())
}

/// The newest event naming a parent wins: an approval answers its asker.
fn parent_override(events: &[BatchEvent]) -> Option<&str> {
    events.iter().rev().find_map(|e| e.parent_session_id.as_deref())
}

#[must_use]
pub fn instruction(preset: &Preset, text: &str) -> String {
    match preset.prompt.trim_end() {
        "" => text.to_owned(),
        prompt => format!("{prompt}\n\n{text}"),
    }
}

#[must_use]
pub fn spawn_request(preset: &Preset, plan: &Plan, prompt: &str) -> Option<SpawnRequest> {
    let parent = match plan {
        Plan::Followup { parent } => Some(parent.as_str()),
        Plan::Spawn | Plan::Reply(_) => None,
    };
    serde_json::from_value(serde_json::json!({
        "machine_id": preset.machine_id?.to_string(),
        "working_dir": preset.working_dir.as_deref()?,
        "adapter_id": preset.adapter_id,
        "model": preset.model,
        "permission_mode": preset.permission_mode,
        "name": preset.name,
        "prompt": prompt,
        "relation": parent.map(|_| crate::followup::RELATION),
        "parent_session_id": parent,
    }))
    .ok()
}

/// No secret is stored: credentials are minted by the dispatch itself.
#[must_use]
pub fn dispatch_request(
    preset: &Preset,
    dispatcher: String,
    batch_id: Uuid,
    prompt: &str,
) -> DispatchRequest {
    DispatchRequest {
        dispatcher,
        session_id: Some(batch_id.to_string()),
        timeout: None,
        reply_url: None,
        notify_url: preset.notify_url.clone(),
        notify_secret: None,
        payload: serde_json::json!({
            "profile": preset.profile,
            "prompt": prompt,
            "name": preset.name,
        }),
        account: None,
        provider: None,
        accounts: Vec::new(),
    }
}

/// A parent that no longer exists degrades to a plain spawn prompt.
pub async fn spawn_prompt(pool: &PgPool, plan: &Plan, instruction: String) -> sqlx::Result<String> {
    let Plan::Followup { parent } = plan else { return Ok(instruction) };
    let rendered = crate::followup::render_followup_prompt(pool, parent, &instruction).await?;
    Ok(rendered.unwrap_or(instruction))
}

/// A webhook delivery's raw payload, attached to the spawn as a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayloadFile {
    pub name: String,
    pub bytes: Vec<u8>,
}

/// Files are attached in order while they fit `budget` bytes in total; a file
/// that does not fit is left out, and so is a second one of the same name.
#[must_use]
pub fn within_budget(files: Vec<PayloadFile>, budget: usize) -> Vec<PayloadFile> {
    let mut kept: Vec<PayloadFile> = Vec::with_capacity(files.len());
    let mut used = 0usize;
    for file in files {
        let fits = used + file.bytes.len() <= budget;
        if fits && kept.iter().all(|k| k.name != file.name) {
            used += file.bytes.len();
            kept.push(file);
        }
    }
    kept
}

pub async fn payload_files(pool: &PgPool, batch_id: Uuid) -> sqlx::Result<Vec<PayloadFile>> {
    let rows: Vec<(Option<String>, serde_json::Value)> = sqlx::query_as(
        "SELECT payload->>'delivery_id', payload->'raw' FROM inbox_events \
         WHERE batch_id = $1 AND source = 'webhook' AND payload ? 'raw' ORDER BY seq",
    )
    .bind(batch_id)
    .fetch_all(pool)
    .await?;
    let files = rows
        .into_iter()
        .map(|(delivery, raw)| PayloadFile {
            name: crate::hooks::payload_file_name(delivery.as_deref().unwrap_or_default()),
            bytes: serde_json::to_vec_pretty(&raw).unwrap_or_default(),
        })
        .collect();
    Ok(within_budget(files, MAX_BATCH_FILE_BYTES))
}

fn uploads(
    files: Vec<PayloadFile>,
) -> (Vec<cctui_proto::adapter::BootstrapFile>, Vec<crate::uploads::RawUpload>) {
    use base64::Engine;
    files
        .into_iter()
        .map(|file| {
            let encoded = cctui_proto::adapter::BootstrapFile {
                name: file.name.clone(),
                content_b64: base64::engine::general_purpose::STANDARD.encode(&file.bytes),
            };
            let raw = crate::uploads::RawUpload {
                name: file.name,
                bytes: file.bytes.into(),
                content_type: Some("application/json".to_owned()),
            };
            (encoded, raw)
        })
        .unzip()
}

/// A scheduled or webhook run carries its preset's name as a label, so
/// `label:` search lists the preset's history. Best-effort, like every label path.
pub(super) async fn run_labels(
    state: &AppState,
    preset: &Preset,
    events: &[BatchEvent],
) -> Vec<String> {
    let organising = state.config.features.contains(cctui_proto::features::Feature::Organising);
    if !organising || !events.iter().any(|e| LABELLED_SOURCES.contains(&e.source.as_str())) {
        return Vec::new();
    }
    let label: sqlx::Result<Uuid> = sqlx::query_scalar(
        "INSERT INTO labels (name, color, user_id) VALUES ($1, $2, $3) \
         ON CONFLICT (user_id, lower(name)) DO UPDATE SET name = labels.name \
         RETURNING id",
    )
    .bind(&preset.name)
    .bind(RUN_LABEL_COLOR)
    .bind(preset.user_id)
    .fetch_one(&state.pool)
    .await;
    match label {
        Ok(id) => vec![id.to_string()],
        Err(e) => {
            tracing::warn!(error = %e, "could not label the scheduled run");
            Vec::new()
        }
    }
}

/// Retried only when nothing reached the daemon; any other failure is checked
/// against `machine_launches` first.
async fn spawn(
    state: &AppState,
    lane: &ClaimedLane,
    preset: &Preset,
    plan: &Plan,
    events: &[BatchEvent],
    files: Vec<PayloadFile>,
) -> sqlx::Result<Outcome> {
    let Some(batch_id) = events.first().map(|e| e.batch_id) else { return Ok(Outcome::Skip) };
    let delivered = |session_id: String, plan: &Plan| Outcome::Delivered {
        session_id,
        delivered_as: plan.delivered_as(),
    };
    match prior_launch(&state.pool, batch_id).await? {
        Prior::Launched(session_id) => {
            set_spawn(&state.pool, lane.id, Some(batch_id)).await?;
            return Ok(delivered(session_id, plan));
        }
        Prior::Refused(detail) => {
            return Ok(Outcome::Dead(format!("the daemon refused the spawn: {detail}")));
        }
        Prior::Undelivered => return Ok(Outcome::Retry(UNDELIVERED.to_owned())),
        Prior::None => {}
    }
    if !preset.machine_live {
        return Ok(Outcome::Dead(MACHINE_GONE.to_owned()));
    }
    let text = match super::deliver::batch_text(events, crate::crypto::vault_key) {
        Ok(text) => text,
        Err(reason) => return Ok(Outcome::Dead(reason.to_owned())),
    };
    let prompt = spawn_prompt(&state.pool, plan, instruction(preset, &text)).await?;
    let Some(mut req) = spawn_request(preset, plan, &prompt) else {
        return Ok(Outcome::Dead(MACHINE_GONE.to_owned()));
    };
    req.label_ids = run_labels(state, preset, events).await;
    let (files, raw_files) = uploads(files);
    let ctx = match super::queued::acting_as(&state.pool, preset.user_id).await {
        Ok(ctx) => ctx,
        Err(e) => return Ok(Outcome::Retry(e.to_string())),
    };
    let sent =
        crate::routes::spawn::try_dispatch_spawn_as(state, &ctx, req, files, raw_files, batch_id)
            .await;
    let failure = match sent {
        Ok((_, Json(launched))) => {
            set_spawn(&state.pool, lane.id, Some(batch_id)).await?;
            let session_id =
                launched.session_id.map_or_else(|| batch_id.to_string(), |id| id.to_string());
            return Ok(delivered(session_id, plan));
        }
        Err(failure) => failure,
    };
    let message = failure.message().to_owned();
    if failure.queue.is_some() {
        return Ok(Outcome::Retry(message));
    }
    Ok(match prior_launch(&state.pool, batch_id).await? {
        Prior::None if failure.response.0.is_server_error() => Outcome::Retry(message),
        Prior::None => Outcome::Dead(message),
        Prior::Refused(detail) => Outcome::Dead(format!("the daemon refused the spawn: {detail}")),
        Prior::Launched(_) | Prior::Undelivered => Outcome::RetryKept(message),
    })
}

/// Containers are one-shot: every batch is its own dispatch, keyed by
/// `batch_id` so the dispatcher's dedup absorbs a redelivery.
async fn dispatch(
    state: &AppState,
    preset: &Preset,
    events: &[BatchEvent],
    batch_id: Uuid,
) -> Outcome {
    let Some(dispatcher) = preset.dispatcher_name.clone() else {
        return Outcome::Dead(DISPATCHER_GONE.to_owned());
    };
    let text = match super::deliver::batch_text(events, crate::crypto::vault_key) {
        Ok(text) => text,
        Err(reason) => return Outcome::Dead(reason.to_owned()),
    };
    let ctx = match super::queued::acting_as(&state.pool, preset.user_id).await {
        Ok(ctx) => ctx,
        Err(e) => return Outcome::Retry(e.to_string()),
    };
    let req = dispatch_request(preset, dispatcher, batch_id, &instruction(preset, &text));
    match cctui_server_core::dispatch::execute_dispatch(state, &ctx, req).await {
        Ok((_, Json(done))) => {
            Outcome::Delivered { session_id: done.session_id, delivered_as: "dispatch" }
        }
        Err((code, Json(e))) if code.is_server_error() => Outcome::RetryKept(e.error),
        Err((_, Json(e))) => Outcome::Dead(e.error),
    }
}

pub async fn deliver(
    state: &AppState,
    lane: &ClaimedLane,
    preset_id: Uuid,
    events: &[BatchEvent],
) -> sqlx::Result<Outcome> {
    let Some(batch_id) = events.first().map(|e| e.batch_id) else { return Ok(Outcome::Skip) };
    let Some(preset) = load(&state.pool, preset_id).await? else {
        return Ok(Outcome::Dead(PRESET_GONE.to_owned()));
    };
    if preset.machine_id.is_none() {
        return Ok(dispatch(state, &preset, events, batch_id).await);
    }
    let current = match resolve(&state.pool, lane).await? {
        Resolved::Wait => return Ok(Outcome::Skip),
        Resolved::Session(current) => current,
    };
    let on_stale = OnStale::parse(&preset.on_stale).unwrap_or_default();
    let files = payload_files(&state.pool, batch_id).await?;
    let planned = plan(preset.fresh_secs, on_stale, current.as_ref(), parent_override(events));
    match with_files(planned, on_stale, !files.is_empty()) {
        Plan::Reply(session_id) => Ok(super::deliver::reply(state, &session_id, events).await),
        plan => spawn(state, lane, &preset, &plan, events, files).await,
    }
}
