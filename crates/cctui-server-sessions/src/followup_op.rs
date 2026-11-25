use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use cctui_proto::adapter::RemoveInitiator;
use cctui_proto::api::SpawnResponse;
use cctui_proto::api::followup::{
    FollowupReason, FollowupRequest, FollowupResponse, FollowupState, IDEMPOTENCY_KEY_MAX,
};
use cctui_proto::ws::ServerEvent;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::AuthContext;
use crate::context::SessionsContext;
use crate::error::AppError;
use crate::events::Actor;
use crate::followup::FollowupPrompt;
use crate::inbox::preset::{MACHINE_GONE, Plan, REGISTER_WAIT_SECS};
use crate::routes::sessions::ArchiveOutcome;
use crate::routes::spawn::SpawnFailure;

pub const BUSY: &str = "a follow-up of this session is in progress";
pub const NOT_REGISTERED: &str = "the follow-up session did not register in time";
pub const PINNED: &str = "the session is pinned, so it was not archived";
const WATCH_EVERY: Duration = Duration::from_millis(500);
const SWEEP_GRACE_SECS: i64 = 60;

macro_rules! op_columns {
    () => {
        "id, source_session_id, owner_id, reason, instruction, archive_source, command_id, \
         target_session_id, state, error, archive_error, archived, registered_at"
    };
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Op {
    pub id: Uuid,
    pub source_session_id: String,
    pub owner_id: Option<Uuid>,
    pub reason: String,
    pub instruction: String,
    pub archive_source: bool,
    pub command_id: Uuid,
    pub target_session_id: Option<String>,
    pub state: String,
    pub error: Option<String>,
    pub archive_error: Option<String>,
    pub archived: bool,
    pub registered_at: Option<DateTime<Utc>>,
}

impl Op {
    #[must_use]
    pub fn state(&self) -> FollowupState {
        FollowupState::parse(&self.state).unwrap_or(FollowupState::Failed)
    }

    #[must_use]
    pub fn reason(&self) -> FollowupReason {
        FollowupReason::parse(&self.reason).unwrap_or(FollowupReason::Manual)
    }

    #[must_use]
    pub fn response(&self) -> FollowupResponse {
        FollowupResponse {
            followup_id: self.id,
            from: self.source_session_id.clone(),
            to: self.target_session_id.clone(),
            state: self.state(),
            error: self.error.clone(),
        }
    }
}

#[must_use]
pub const fn reason_name(reason: FollowupReason) -> &'static str {
    match reason {
        FollowupReason::Cold => "cold",
        FollowupReason::Manual => "manual",
        FollowupReason::Auto => "auto",
        FollowupReason::Inbox => "inbox",
    }
}

#[derive(Debug, Clone)]
pub struct NewOp<'a> {
    pub source: &'a str,
    pub owner: Option<Uuid>,
    pub key: &'a str,
    pub reason: FollowupReason,
    pub instruction: &'a str,
    pub archive_source: bool,
    pub command_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Begun {
    Started(Op),
    Existing(Op),
    Busy(Op),
    NoSource,
}

pub async fn op_by_key(pool: &PgPool, source: &str, key: &str) -> sqlx::Result<Option<Op>> {
    sqlx::query_as(concat!(
        "SELECT ",
        op_columns!(),
        " FROM session_followup_ops WHERE source_session_id = $1 AND idempotency_key = $2"
    ))
    .bind(source)
    .bind(key)
    .fetch_optional(pool)
    .await
}

async fn pending_op(pool: &PgPool, source: &str) -> sqlx::Result<Option<Op>> {
    sqlx::query_as(concat!(
        "SELECT ",
        op_columns!(),
        " FROM session_followup_ops WHERE source_session_id = $1 AND state = 'pending'"
    ))
    .bind(source)
    .fetch_optional(pool)
    .await
}

pub async fn load(pool: &PgPool, id: Uuid) -> sqlx::Result<Option<Op>> {
    sqlx::query_as(concat!("SELECT ", op_columns!(), " FROM session_followup_ops WHERE id = $1"))
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn begin(pool: &PgPool, new: &NewOp<'_>) -> sqlx::Result<Begun> {
    for _ in 0..3 {
        let inserted: Option<Op> = sqlx::query_as(concat!(
            "INSERT INTO session_followup_ops (source_session_id, owner_id, idempotency_key, \
                 reason, instruction, archive_source, command_id) \
             SELECT $1, $2, $3, $4, $5, $6, $7 \
              WHERE EXISTS (SELECT 1 FROM sessions WHERE id = $1) \
             ON CONFLICT DO NOTHING RETURNING ",
            op_columns!()
        ))
        .bind(new.source)
        .bind(new.owner)
        .bind(new.key)
        .bind(reason_name(new.reason))
        .bind(new.instruction)
        .bind(new.archive_source)
        .bind(new.command_id)
        .fetch_optional(pool)
        .await?;
        if let Some(op) = inserted {
            return Ok(Begun::Started(op));
        }
        if let Some(op) = op_by_key(pool, new.source, new.key).await? {
            return Ok(Begun::Existing(op));
        }
        if let Some(op) = pending_op(pool, new.source).await? {
            return Ok(Begun::Busy(op));
        }
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM sessions WHERE id = $1)")
                .bind(new.source)
                .fetch_one(pool)
                .await?;
        if !exists {
            return Ok(Begun::NoSource);
        }
    }
    Err(sqlx::Error::Protocol("the follow-up kept racing another one of its session".into()))
}

pub async fn in_progress(pool: &PgPool, session_id: &str) -> sqlx::Result<bool> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM session_followup_ops \
         WHERE source_session_id = $1 AND state = 'pending')",
    )
    .bind(session_id)
    .fetch_one(pool)
    .await
}

pub async fn refuse_while_pending(pool: &PgPool, session_id: &str) -> Result<(), AppError> {
    if in_progress(pool, session_id).await? {
        return Err(AppError::new(StatusCode::CONFLICT, BUSY));
    }
    Ok(())
}

pub async fn launched<S: SessionsContext>(
    state: &S,
    id: Uuid,
    launched: &SpawnResponse,
) -> sqlx::Result<()> {
    let spawn_key = launched.session_id.unwrap_or(launched.command_id).to_string();
    sqlx::query(
        "UPDATE session_followup_ops SET spawn_key = $2, \
             target_session_id = COALESCE(target_session_id, $3), updated_at = now() \
         WHERE id = $1",
    )
    .bind(id)
    .bind(&spawn_key)
    .bind(launched.session_id.map(|s| s.to_string()))
    .execute(state.pool())
    .await?;
    watch(state, id);
    Ok(())
}

/// The registering session takes over the pending op its spawn started.
/// Matched by the spawn key it echoes or through its launch row, so a daemon
/// faster than [`launched`] still resolves.
pub async fn claim(pool: &PgPool, session_id: &str, spawn_key: Option<&str>) {
    crate::auto_followup::claim_intent(pool, session_id, spawn_key).await;
    let key = spawn_key.unwrap_or(session_id);
    let claimed: sqlx::Result<Option<(Uuid, String, String)>> = sqlx::query_as(
        "UPDATE session_followup_ops SET state = 'registered', target_session_id = $1, \
             registered_at = now(), updated_at = now() \
         WHERE state = 'pending' AND source_session_id <> $1 \
           AND (spawn_key IN ($1, $2) OR command_id IN ( \
                SELECT command_id FROM machine_launches \
                 WHERE session_id IN ($1, $2) OR registered_session_id = $1)) \
         RETURNING id, reason, instruction",
    )
    .bind(session_id)
    .bind(key)
    .fetch_optional(pool)
    .await;
    let (id, reason, instruction) = match claimed {
        Ok(Some(row)) => row,
        Ok(None) => return,
        Err(e) => {
            tracing::warn!(%session_id, error = %e, "follow-up op claim failed");
            return;
        }
    };
    if let Err(e) = sqlx::query(
        "UPDATE sessions SET metadata = COALESCE(metadata, '{}'::jsonb) || jsonb_build_object( \
             'followup_reason', $2::text, 'followup_instruction', $3::text, \
             'followup_id', $4::text) \
         WHERE id = $1",
    )
    .bind(session_id)
    .bind(&reason)
    .bind(&instruction)
    .bind(id.to_string())
    .execute(pool)
    .await
    {
        tracing::warn!(%session_id, followup = %id, error = %e, "could not mark the follow-up");
    }
}

pub async fn fail<S: SessionsContext>(
    state: &S,
    id: Uuid,
    error: &str,
) -> sqlx::Result<Option<Op>> {
    let failed: Option<Op> = sqlx::query_as(concat!(
        "UPDATE session_followup_ops SET state = 'failed', error = $2, settled_at = now(), \
             updated_at = now() \
         WHERE id = $1 AND state = 'pending' RETURNING ",
        op_columns!()
    ))
    .bind(id)
    .bind(error)
    .fetch_optional(state.pool())
    .await?;
    if let Some(op) = &failed {
        publish_failed(state, op);
    }
    Ok(failed)
}

fn publish_failed<S: SessionsContext>(state: &S, op: &Op) {
    tracing::info!(followup = %op.id, from = %op.source_session_id, "follow-up failed");
    state.bus().publish_server(ServerEvent::FollowupFailed {
        followup_id: op.id,
        from: op.source_session_id.clone(),
        reason: op.reason(),
        instruction: op.instruction.clone(),
        error: op.error.clone().unwrap_or_default(),
    });
}

async fn archive_source<S: SessionsContext>(state: &S, op: &Op) -> (bool, Option<String>) {
    let (initiator, actor) = match (op.reason(), op.owner_id) {
        (FollowupReason::Cold | FollowupReason::Manual, Some(owner)) => {
            (RemoveInitiator::User, Actor::User(owner))
        }
        _ => (RemoveInitiator::Automatic, Actor::System),
    };
    match crate::routes::sessions::archive_one(
        state,
        &op.source_session_id,
        false,
        initiator,
        actor,
    )
    .await
    {
        Ok(ArchiveOutcome::Archived) => (true, None),
        Ok(ArchiveOutcome::SkippedPinned) => (false, Some(PINNED.to_owned())),
        Err(e) => (false, Some(e.to_string())),
    }
}

/// Claims the op first, so a watcher and the sweep never both archive or announce.
pub async fn settle<S: SessionsContext>(state: &S, id: Uuid) -> sqlx::Result<bool> {
    let claimed: Option<Op> = sqlx::query_as(concat!(
        "UPDATE session_followup_ops SET settled_at = now(), updated_at = now() \
         WHERE id = $1 AND state = 'registered' AND settled_at IS NULL RETURNING ",
        op_columns!()
    ))
    .bind(id)
    .fetch_optional(state.pool())
    .await?;
    let Some(op) = claimed else { return Ok(false) };
    let (archived, archive_error) =
        if op.archive_source { archive_source(state, &op).await } else { (false, None) };
    sqlx::query(
        "UPDATE session_followup_ops SET archived = $2, archive_error = $3, updated_at = now() \
         WHERE id = $1",
    )
    .bind(op.id)
    .bind(archived)
    .bind(&archive_error)
    .execute(state.pool())
    .await?;
    let to = op.target_session_id.clone().unwrap_or_default();
    tracing::info!(followup = %op.id, from = %op.source_session_id, %to, archived, "superseded");
    state.bus().publish_server(ServerEvent::SessionSuperseded {
        followup_id: op.id,
        from: op.source_session_id.clone(),
        to,
        reason: op.reason(),
        instruction: op.instruction.clone(),
        archived,
        archive_error,
        at: op.registered_at.unwrap_or_else(Utc::now),
    });
    Ok(true)
}

/// Poll the op until it registers or the spawn timeout passes. The daemon may
/// register on another replica, so the row, not this process, is the signal.
pub fn watch<S: SessionsContext>(state: &S, id: Uuid) {
    let state = state.clone();
    let wait = Duration::from_secs(REGISTER_WAIT_SECS.unsigned_abs());
    tokio::spawn(async move {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            tokio::time::sleep(WATCH_EVERY).await;
            match load(state.pool(), id).await {
                Ok(Some(op)) if op.state() == FollowupState::Registered => {
                    if let Err(e) = settle(&state, id).await {
                        tracing::warn!(followup = %id, error = %e, "settling the follow-up failed");
                    }
                    return;
                }
                Ok(Some(op)) if op.state() == FollowupState::Pending => {}
                Ok(_) => return,
                Err(e) => tracing::warn!(followup = %id, error = %e, "follow-up poll failed"),
            }
            if tokio::time::Instant::now() >= deadline {
                if let Err(e) = fail(&state, id, NOT_REGISTERED).await {
                    tracing::warn!(followup = %id, error = %e, "failing the follow-up failed");
                }
                return;
            }
        }
    });
}

/// Finishes the ops whose watcher died with its replica.
pub async fn sweep<S: SessionsContext>(state: &S) {
    let timed_out: sqlx::Result<Vec<Op>> = sqlx::query_as(concat!(
        "UPDATE session_followup_ops SET state = 'failed', error = $2, settled_at = now(), \
             updated_at = now() \
         WHERE state = 'pending' AND created_at < now() - $1::bigint * interval '1 second' \
         RETURNING ",
        op_columns!()
    ))
    .bind(REGISTER_WAIT_SECS + SWEEP_GRACE_SECS)
    .bind(NOT_REGISTERED)
    .fetch_all(state.pool())
    .await;
    match timed_out {
        Ok(ops) => {
            for op in &ops {
                publish_failed(state, op);
            }
        }
        Err(e) => tracing::warn!(error = %e, "follow-up op sweep failed"),
    }
    let unsettled: sqlx::Result<Vec<Uuid>> = sqlx::query_scalar(
        "SELECT id FROM session_followup_ops \
         WHERE state = 'registered' AND settled_at IS NULL \
           AND registered_at < now() - $1::bigint * interval '1 second'",
    )
    .bind(SWEEP_GRACE_SECS)
    .fetch_all(state.pool())
    .await;
    for id in unsettled.unwrap_or_default() {
        if let Err(e) = settle(state, id).await {
            tracing::warn!(followup = %id, error = %e, "settling a left-over follow-up failed");
        }
    }
}

/// The parent's name with its trailing counter bumped, or ` 2` appended. Only
/// a name the user set carries over.
#[must_use]
pub fn followup_name(name: Option<&str>, user_named: bool) -> Option<String> {
    let name = name.map(str::trim).filter(|n| user_named && !n.is_empty())?;
    if let Some((head, n)) = name.rsplit_once(' ')
        && !head.is_empty()
        && !head.ends_with(char::is_whitespace)
        && (1..=3).contains(&n.len())
        && !n.starts_with('0')
        && n.bytes().all(|b| b.is_ascii_digit())
        && let Ok(count) = n.parse::<u32>()
    {
        return Some(format!("{head} {}", count + 1));
    }
    Some(format!("{name} 2"))
}

#[derive(Debug, Default, sqlx::FromRow)]
struct Carried {
    session_name: Option<String>,
    user_named: bool,
    effort: Option<String>,
    label_ids: Vec<String>,
}

async fn carried(pool: &PgPool, source: &str) -> sqlx::Result<Carried> {
    let found: Option<Carried> = sqlx::query_as(
        "SELECT s.session_name, COALESCE(s.metadata->>'name_source', '') = 'user' AS user_named, \
                s.effort, \
                ARRAY(SELECT l.label_id::text FROM session_labels l \
                      WHERE l.session_id = s.id ORDER BY l.created_at) AS label_ids \
         FROM sessions s WHERE s.id = $1",
    )
    .bind(source)
    .fetch_optional(pool)
    .await?;
    Ok(found.unwrap_or_default())
}

fn ask(instruction: &str) -> String {
    match instruction.trim() {
        "" => crate::followup::DEFAULT_INSTRUCTION.to_owned(),
        ask => ask.to_owned(),
    }
}

type Prepared = (
    cctui_proto::api::SpawnRequest,
    Vec<cctui_proto::adapter::BootstrapFile>,
    Vec<crate::uploads::RawUpload>,
);

async fn prepare(
    pool: &PgPool,
    ctx: &AuthContext,
    source: &str,
    instruction: &str,
) -> Result<Prepared, AppError> {
    let owner = crate::authz::session_owner(source, pool).await?.unwrap_or(ctx.user_id);
    let Some(target) = crate::inbox::preset::from_session(pool, owner, source).await? else {
        return Err(AppError::new(StatusCode::NOT_FOUND, "session not found"));
    };
    if !target.machine_live {
        return Err(AppError::new(StatusCode::CONFLICT, MACHINE_GONE));
    }
    let FollowupPrompt { prompt, file } =
        crate::followup::followup_for(pool, crate::brief::Reader::Caller(ctx), source, instruction)
            .await?
            .unwrap_or_else(|| FollowupPrompt { prompt: ask(instruction), file: None });
    let plan = Plan::Followup { parent: source.to_owned() };
    let Some(mut req) = crate::inbox::preset::spawn_request(&target, &plan, &prompt) else {
        return Err(AppError::new(StatusCode::CONFLICT, MACHINE_GONE));
    };
    let carried = carried(pool, source).await?;
    req.name = followup_name(carried.session_name.as_deref(), carried.user_named);
    req.effort = carried.effort;
    req.label_ids = carried.label_ids;
    let (files, raw) = file.map(crate::inbox::preset::brief_upload).into_iter().unzip();
    Ok((req, files, raw))
}

async fn launch<S: SessionsContext>(
    state: &S,
    ctx: &AuthContext,
    op: &Op,
) -> Result<SpawnResponse, SpawnFailure> {
    let (req, files, raw) =
        prepare(state.pool(), ctx, &op.source_session_id, &op.instruction).await?;
    let (_, Json(launched)) =
        crate::routes::spawn::try_dispatch_spawn_as(state, ctx, req, files, raw, op.command_id)
            .await?;
    Ok(launched)
}

fn validated_key(raw: &str) -> Result<&str, AppError> {
    let key = raw.trim();
    if key.is_empty() || key.len() > IDEMPOTENCY_KEY_MAX {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            format!("idempotency_key must be 1 to {IDEMPOTENCY_KEY_MAX} characters"),
        ));
    }
    Ok(key)
}

pub async fn start<S: SessionsContext>(
    state: &S,
    ctx: &AuthContext,
    source: &str,
    req: &FollowupRequest,
) -> Result<(StatusCode, FollowupResponse), AppError> {
    let key = validated_key(&req.idempotency_key)?;
    let new = NewOp {
        source,
        owner: crate::authz::session_owner(source, state.pool()).await?,
        key,
        reason: req.reason,
        instruction: req.instruction.trim(),
        archive_source: req.archive_source,
        command_id: Uuid::new_v4(),
    };
    let op = match begin(state.pool(), &new).await? {
        Begun::Started(op) => op,
        Begun::Existing(op) => return Ok((StatusCode::OK, op.response())),
        Begun::Busy(op) => return Ok((StatusCode::CONFLICT, op.response())),
        Begun::NoSource => return Err(AppError::new(StatusCode::NOT_FOUND, "session not found")),
    };
    match launch(state, ctx, &op).await {
        Ok(spawned) => {
            launched(state, op.id, &spawned).await?;
            let mut response = op.response();
            response.to = spawned.session_id.map(|s| s.to_string());
            Ok((StatusCode::ACCEPTED, response))
        }
        Err(failure) => {
            let (code, Json(e)) = failure.response;
            fail(state, op.id, &e.error).await?;
            Err(AppError::new(code, e.error))
        }
    }
}

pub async fn create<S: SessionsContext>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
    Path(source): Path<String>,
    Json(req): Json<FollowupRequest>,
) -> Result<Response, AppError> {
    let (code, body) = start(&state, &ctx, &source, &req).await?;
    Ok((code, Json(body)).into_response())
}

#[cfg(test)]
#[path = "followup_op_tests.rs"]
mod tests;
