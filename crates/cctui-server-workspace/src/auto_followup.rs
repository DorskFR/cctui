use std::collections::HashMap;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use cctui_proto::api::SessionListItem;
use cctui_proto::api::auto_followup::{
    AutoFollowupOverride, AutoFollowupSetting, ContextWindow, SendFollowup, WindowHints,
    context_window,
};
use cctui_proto::api::followup::{
    FollowupReason, FollowupRequest, FollowupState, IDEMPOTENCY_KEY_MAX,
};
use cctui_proto::classifier::Bucket;
use cctui_proto::features::Feature;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::AuthContext;
use crate::error::AppError;
use crate::followup::nests_under_parent;
use crate::state::AppState;
use crate::store::sessions::SessionRowStatus;

pub const SETTING_KEY: &str = "autoFollowup";
pub const METADATA_KEY: &str = "auto_followup";
pub const THRESHOLD_OUT_OF_RANGE: &str =
    "an auto follow-up threshold is 1 to 100 percent, or more than 0 tokens";
const CODEX_WINDOW_KEY: &str = "model_context_window";

type PrefsRow = (String, Option<Value>, Value, bool, Option<String>);

#[must_use]
pub fn setting_of(data: &Value) -> AutoFollowupSetting {
    data.get(SETTING_KEY)
        .and_then(|v| <AutoFollowupSetting as serde::Deserialize>::deserialize(v).ok())
        .filter(|s| s.threshold.valid())
        .unwrap_or_default()
}

#[must_use]
pub fn override_of(metadata: &Value) -> Option<AutoFollowupOverride> {
    metadata
        .get(METADATA_KEY)
        .and_then(|v| <AutoFollowupOverride as serde::Deserialize>::deserialize(v).ok())
        .filter(|o| !o.is_empty() && o.valid())
}

#[must_use]
pub fn window_of(
    adapter_id: Option<&str>,
    model: Option<&str>,
    launched_model: Option<&str>,
    metadata: &Value,
    observed_tokens: Option<u64>,
) -> Option<ContextWindow> {
    let hints = WindowHints {
        reported_model: model.or_else(|| metadata.get("model").and_then(Value::as_str)),
        launched_model,
        codex_window: metadata.get(CODEX_WINDOW_KEY).and_then(Value::as_u64),
        observed_tokens,
    };
    context_window(adapter_id, &hints)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hold {
    Closed,
    Pinned,
    Child,
    Busy,
    Waiting,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Signals<'a> {
    pub status: &'a str,
    pub ended: bool,
    pub pinned: bool,
    pub child: bool,
    pub tempo: Option<&'a str>,
    pub agent_state: Option<&'a str>,
    pub activity: Option<&'a str>,
    pub soft_limit: Option<&'a str>,
}

#[must_use]
pub fn hold(signals: &Signals<'_>) -> Option<Hold> {
    let closed = signals.ended
        || SessionRowStatus::parse(signals.status).is_some_and(|s| {
            matches!(
                s,
                SessionRowStatus::Archived
                    | SessionRowStatus::Ended
                    | SessionRowStatus::Failed
                    | SessionRowStatus::Draft
                    | SessionRowStatus::Queued
            )
        });
    if closed {
        return Some(Hold::Closed);
    }
    if signals.pinned {
        return Some(Hold::Pinned);
    }
    if signals.child {
        return Some(Hold::Child);
    }
    let bucket = crate::routes::sessions::bucket_from_signals(
        signals.tempo,
        signals.agent_state,
        signals.activity,
        signals.soft_limit,
        &[],
        &HashMap::new(),
    );
    match bucket {
        Bucket::Working => Some(Hold::Busy),
        Bucket::Blocked => Some(Hold::Waiting),
        Bucket::Done => None,
    }
}

#[must_use]
pub const fn threshold_at(
    setting: AutoFollowupSetting,
    window: Option<ContextWindow>,
    held: Option<Hold>,
) -> Option<u64> {
    if !setting.enabled || held.is_some() {
        return None;
    }
    let base = match window {
        Some(window) => window.for_percent(),
        None => None,
    };
    setting.threshold.tokens(base)
}

#[must_use]
pub fn fires(followup: SendFollowup, text: &str, context: Option<u64>, at: Option<u64>) -> bool {
    followup.is_auto()
        && !text.trim().is_empty()
        && context.zip(at).is_some_and(|(context, at)| context >= at)
}

/// The stored half of a session's auto follow-up: the live registry's
/// metadata copy can lag the row.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Prefs {
    pub user: AutoFollowupSetting,
    pub over: Option<AutoFollowupOverride>,
    pub child: bool,
    pub launched_model: Option<String>,
}

pub fn fill(
    item: &mut SessionListItem,
    prefs: Option<Prefs>,
    turn: Option<Hold>,
    automation: bool,
) {
    let adapter = item.adapter_id.as_ref().map(cctui_proto::adapter::AdapterId::as_str);
    let launched = prefs.as_ref().and_then(|p| p.launched_model.as_deref());
    let model = item.model.as_deref();
    let window = window_of(adapter, model, launched, &item.metadata, item.context_tokens);
    item.context_window = window.map(|w| w.tokens);
    let Some(prefs) = prefs.filter(|_| automation) else { return };
    let held = if prefs.child { Some(Hold::Child) } else { turn };
    item.auto_followup_override = prefs.over;
    let setting = prefs.user.with(prefs.over.as_ref());
    item.auto_followup_at = threshold_at(setting, window, held);
}

/// The model a session was launched with: the harness reports it without
/// its `[1m]` suffix. Claude pre-mints its id, so the launch row's
/// `session_id` is the session's.
macro_rules! launched_model_sql {
    () => {
        "(SELECT ml.model FROM machine_launches ml WHERE ml.session_id = s.id)"
    };
}

const PREFS_SQL: &str = concat!(
    "SELECT s.id, us.data, COALESCE(s.metadata, '{}'::jsonb), s.parent_id IS NOT NULL AND ",
    nests_under_parent!("s"),
    ", ",
    launched_model_sql!(),
    " FROM sessions s LEFT JOIN user_settings us ON us.user_id = s.user_id WHERE s.id = ANY($1)"
);

pub async fn prefs_for(
    pool: &PgPool,
    session_ids: &[String],
) -> sqlx::Result<HashMap<String, Prefs>> {
    let rows: Vec<PrefsRow> =
        sqlx::query_as(PREFS_SQL).bind(session_ids).fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(|(id, data, metadata, child, launched_model)| {
            let user = data.as_ref().map(setting_of).unwrap_or_default();
            (id, Prefs { user, over: override_of(&metadata), child, launched_model })
        })
        .collect())
}

#[derive(Debug, sqlx::FromRow)]
struct MeasureRow {
    user_id: Option<Uuid>,
    status: String,
    tempo: Option<String>,
    agent_state: Option<String>,
    activity: Option<String>,
    soft_limit_reason: Option<String>,
    pinned: bool,
    child: bool,
    ended: bool,
    adapter_id: Option<String>,
    model: Option<String>,
    launched_model: Option<String>,
    metadata: Value,
    settings: Option<Value>,
    context_tokens: Option<i64>,
}

const MEASURE_SQL: &str = concat!(
    "SELECT s.user_id, s.status, s.tempo, s.agent_state, s.activity, s.soft_limit_reason, \
            s.pinned, s.parent_id IS NOT NULL AND ",
    nests_under_parent!("s"),
    " AS child, s.ended_at IS NOT NULL AS ended, s.adapter_id, s.model, ",
    launched_model_sql!(),
    " AS launched_model, COALESCE(s.metadata, '{}'::jsonb) AS metadata, us.data AS settings, \
            (SELECT u.input_tokens + u.cache_read_tokens + u.cache_creation_tokens \
               FROM session_token_usage u WHERE u.session_id = s.id \
              ORDER BY u.created_at DESC LIMIT 1) AS context_tokens \
     FROM sessions s LEFT JOIN user_settings us ON us.user_id = s.user_id WHERE s.id = $1"
);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Measure {
    pub owner: Option<Uuid>,
    pub context_tokens: Option<u64>,
    pub context_window: Option<u64>,
    pub over: Option<AutoFollowupOverride>,
    pub held: Option<Hold>,
    pub at: Option<u64>,
}

pub async fn measure(pool: &PgPool, session_id: &str) -> sqlx::Result<Option<Measure>> {
    let row: Option<MeasureRow> =
        sqlx::query_as(MEASURE_SQL).bind(session_id).fetch_optional(pool).await?;
    Ok(row.map(|row| {
        let held = hold(&Signals {
            status: &row.status,
            ended: row.ended,
            pinned: row.pinned,
            child: row.child,
            tempo: row.tempo.as_deref(),
            agent_state: row.agent_state.as_deref(),
            activity: row.activity.as_deref(),
            soft_limit: row.soft_limit_reason.as_deref(),
        });
        let context_tokens = row.context_tokens.and_then(|t| u64::try_from(t).ok());
        let window = window_of(
            row.adapter_id.as_deref(),
            row.model.as_deref(),
            row.launched_model.as_deref(),
            &row.metadata,
            context_tokens,
        );
        let user = row.settings.as_ref().map(setting_of).unwrap_or_default();
        let over = override_of(&row.metadata);
        Measure {
            owner: row.user_id,
            context_tokens,
            context_window: window.map(|w| w.tokens),
            over,
            held,
            at: threshold_at(user.with(over.as_ref()), window, held),
        }
    }))
}

pub async fn fill_detail(state: &AppState, item: &mut SessionListItem) -> sqlx::Result<()> {
    let Some(measured) = measure(&state.pool, &item.id).await? else { return Ok(()) };
    item.context_tokens = measured.context_tokens;
    item.context_window = measured.context_window;
    if state.config.features.contains(Feature::Automation) {
        item.auto_followup_override = measured.over;
        item.auto_followup_at = measured.at;
    }
    Ok(())
}

#[must_use]
pub fn key_for(message_key: Option<&str>) -> String {
    message_key
        .map(str::trim)
        .filter(|k| !k.is_empty() && k.len() + 5 <= IDEMPOTENCY_KEY_MAX)
        .map_or_else(|| format!("auto:{}", Uuid::new_v4()), |k| format!("auto:{k}"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Followed {
    pub followup_id: Uuid,
    pub command_id: Uuid,
    pub to: Option<String>,
}

/// `None` = send plainly, also when the follow-up could not start, so the
/// message is never lost to it.
pub async fn route(
    state: &AppState,
    caller: Option<&AuthContext>,
    session_id: &str,
    followup: SendFollowup,
    text: &str,
    message_key: Option<&str>,
) -> Result<Option<Followed>, AppError> {
    if !followup.is_auto()
        || text.trim().is_empty()
        || !state.config.features.contains(Feature::Automation)
    {
        return Ok(None);
    }
    let measured = match measure(&state.pool, session_id).await {
        Ok(Some(m)) => m,
        Ok(None) => return Ok(None),
        Err(e) => {
            tracing::warn!(%session_id, error = %e, "auto follow-up check failed");
            return Ok(None);
        }
    };
    if !fires(followup, text, measured.context_tokens, measured.at) {
        return Ok(None);
    }
    let ctx = if let Some(ctx) = caller {
        ctx.clone()
    } else {
        let Some(owner) = measured.owner else { return Ok(None) };
        crate::inbox::queued::acting_as(&state.pool, owner).await?
    };
    let req = FollowupRequest {
        instruction: text.to_owned(),
        reason: FollowupReason::Auto,
        archive_source: true,
        idempotency_key: key_for(message_key),
    };
    let (code, res) = match crate::followup_op::start(state, &ctx, session_id, &req).await {
        Ok(started) => started,
        Err(e) => {
            tracing::warn!(%session_id, error = %e, "auto follow-up did not start, sending here");
            return Ok(None);
        }
    };
    if code == StatusCode::CONFLICT {
        return Err(AppError::new(StatusCode::CONFLICT, crate::followup_op::BUSY));
    }
    if res.state == FollowupState::Failed {
        return Ok(None);
    }
    let op = crate::followup_op::load(&state.pool, res.followup_id).await?;
    tracing::info!(
        %session_id,
        followup = %res.followup_id,
        context = measured.context_tokens,
        at = measured.at,
        "auto follow-up"
    );
    Ok(Some(Followed {
        followup_id: res.followup_id,
        command_id: op.map_or(res.followup_id, |op| op.command_id),
        to: res.to,
    }))
}

#[must_use]
pub fn spawn_response(followed: &Followed) -> cctui_proto::api::SpawnResponse {
    cctui_proto::api::SpawnResponse {
        command_id: followed.command_id,
        status: "followup".into(),
        account: None,
        session_id: followed.to.as_deref().and_then(|to| Uuid::parse_str(to).ok()),
    }
}

/// The profile's override, else a follow-up's source's.
pub async fn spawn_override(
    pool: &PgPool,
    req: &cctui_proto::api::SpawnRequest,
    profile_owner: Uuid,
) -> sqlx::Result<Option<AutoFollowupOverride>> {
    if let Some(profile) = req.profile_id {
        let stored: Option<Option<Value>> = sqlx::query_scalar(
            "SELECT auto_followup FROM session_profiles WHERE id = $1 AND user_id = $2",
        )
        .bind(profile)
        .bind(profile_owner)
        .fetch_optional(pool)
        .await?;
        let over: Option<AutoFollowupOverride> =
            stored.flatten().and_then(|v| serde_json::from_value(v).ok());
        if let Some(over) = over {
            return Ok(Some(over).filter(|o| !o.is_empty() && o.valid()));
        }
    }
    let Some(parent) = crate::followup::parent_of(req) else { return Ok(None) };
    let metadata: Option<Value> =
        sqlx::query_scalar("SELECT COALESCE(metadata, '{}'::jsonb) FROM sessions WHERE id = $1")
            .bind(parent)
            .fetch_optional(pool)
            .await?;
    Ok(metadata.as_ref().and_then(override_of))
}

/// Best-effort: a lost intent leaves the session on the user setting.
pub async fn remember_intent(
    pool: &PgPool,
    spawn_key: &str,
    req: &cctui_proto::api::SpawnRequest,
    profile_owner: Uuid,
) {
    let over = match spawn_override(pool, req, profile_owner).await {
        Ok(Some(over)) => over,
        Ok(None) => return,
        Err(e) => {
            tracing::warn!(%spawn_key, error = %e, "auto follow-up override lookup failed");
            return;
        }
    };
    if let Err(e) = sqlx::query(
        "INSERT INTO session_auto_followup_intents (spawn_key, auto_followup) VALUES ($1, $2) \
         ON CONFLICT (spawn_key) DO UPDATE SET auto_followup = EXCLUDED.auto_followup",
    )
    .bind(spawn_key)
    .bind(sqlx::types::Json(over))
    .execute(pool)
    .await
    {
        tracing::warn!(%spawn_key, error = %e, "could not record the auto follow-up override");
    }
}

pub async fn claim_intent(pool: &PgPool, session_id: &str, spawn_key: Option<&str>) {
    let key = spawn_key.unwrap_or(session_id);
    let claimed = sqlx::query(
        "WITH intent AS ( \
             DELETE FROM session_auto_followup_intents \
              WHERE spawn_key IN ($1, $2) OR created_at < now() - interval '6 hours' \
              RETURNING spawn_key, auto_followup) \
         UPDATE sessions SET metadata = COALESCE(metadata, '{}'::jsonb) \
                || jsonb_build_object($3::text, i.auto_followup) \
           FROM (SELECT auto_followup FROM intent WHERE spawn_key IN ($1, $2) LIMIT 1) i \
          WHERE id = $1",
    )
    .bind(session_id)
    .bind(key)
    .bind(METADATA_KEY)
    .execute(pool)
    .await;
    if let Err(e) = claimed {
        tracing::warn!(%session_id, error = %e, "auto follow-up override claim failed");
    }
}

pub async fn store_override(
    pool: &PgPool,
    session_id: &str,
    over: &AutoFollowupOverride,
) -> sqlx::Result<bool> {
    let done = if over.is_empty() {
        sqlx::query(
            "UPDATE sessions SET metadata = COALESCE(metadata, '{}'::jsonb) - $2 WHERE id = $1",
        )
        .bind(session_id)
        .bind(METADATA_KEY)
        .execute(pool)
        .await?
    } else {
        sqlx::query(
            "UPDATE sessions SET metadata = COALESCE(metadata, '{}'::jsonb) \
                || jsonb_build_object($2::text, $3::jsonb) WHERE id = $1",
        )
        .bind(session_id)
        .bind(METADATA_KEY)
        .bind(sqlx::types::Json(over))
        .execute(pool)
        .await?
    };
    Ok(done.rows_affected() > 0)
}

/// `PUT /sessions/{id}/auto-followup`: both fields null clears the override.
pub async fn put_override(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(over): Json<AutoFollowupOverride>,
) -> Result<Json<SessionListItem>, AppError> {
    if !over.valid() {
        return Err(AppError::new(StatusCode::BAD_REQUEST, THRESHOLD_OUT_OF_RANGE));
    }
    if !store_override(&state.pool, &session_id, &over).await? {
        return Err(AppError::new(StatusCode::NOT_FOUND, "session not found"));
    }
    crate::routes::sessions::get_session(State(state), Path(session_id)).await
}

#[cfg(test)]
#[path = "auto_followup_tests.rs"]
mod tests;
