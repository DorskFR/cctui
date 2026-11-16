use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::auth::{AuthContext, Scope};
use crate::error::AppError;
use crate::inbox::store::{self, Cancel, Event, NewEvent};
use crate::inbox::{Source, Status};
use crate::scheduled_messages::MAX_HORIZON_DAYS;
use crate::state::AppState;

type ApiResult<T> = Result<T, AppError>;
type Window = (Option<DateTime<Utc>>, Option<DateTime<Utc>>);

const MAX_BODY_BYTES: usize = 64 * 1024;
const MAX_PAYLOAD_BYTES: usize = 8 * 1024;
const MAX_SOURCE_ID_CHARS: usize = 256;
const SECRET_KEYS: &[&str] = &[
    "env",
    "secret",
    "secrets",
    "token",
    "password",
    "api_key",
    "apikey",
    "authorization",
    "cookie",
    "reply_url",
    "notify_secret",
];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateEvent {
    pub session_id: Option<String>,
    pub preset: Option<String>,
    pub topic: Option<String>,
    pub source: String,
    pub source_id: Option<String>,
    pub body: String,
    pub payload: Option<Value>,
    pub not_before: Option<String>,
    pub expires_at: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ValidEvent {
    pub session_id: String,
    pub source: Source,
    pub source_id: Option<String>,
    pub body: String,
    pub payload: Option<Value>,
    pub not_before: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateEventResponse {
    pub id: Uuid,
    pub status: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct ListQuery {
    pub lane: Option<String>,
    pub status: Option<String>,
}

fn bad_request(msg: impl Into<String>) -> AppError {
    AppError::new(StatusCode::BAD_REQUEST, msg)
}

fn not_found() -> AppError {
    AppError::new(StatusCode::NOT_FOUND, "no inbox event with that id")
}

fn target(req: &CreateEvent) -> Result<String, String> {
    if req.preset.is_some() {
        return Err("preset targets are not supported yet: pass session_id".into());
    }
    if req.topic.is_some() {
        return Err("topic applies to preset targets only".into());
    }
    match req.session_id.as_deref().map(str::trim) {
        Some(id) if !id.is_empty() => Ok(id.to_owned()),
        _ => Err("session_id is required".into()),
    }
}

fn source_id(source: Source, raw: Option<&str>) -> Result<Option<String>, String> {
    let id = raw.map(str::trim).filter(|id| !id.is_empty());
    match id {
        None if source.requires_source_id() => Err(format!(
            "source_id is required for source '{}': pass the upstream delivery id",
            source.as_str()
        )),
        Some(id) if id.chars().count() > MAX_SOURCE_ID_CHARS => {
            Err(format!("source_id must be at most {MAX_SOURCE_ID_CHARS} characters"))
        }
        id => Ok(id.map(ToOwned::to_owned)),
    }
}

fn secret_key(value: &Value) -> Option<&str> {
    match value {
        Value::Object(map) => map.iter().find_map(|(key, inner)| {
            let lower = key.to_ascii_lowercase();
            let secret = SECRET_KEYS.contains(&lower.as_str())
                || ["_secret", "_token", "_password"].iter().any(|&s| lower.ends_with(s));
            if secret { Some(key.as_str()) } else { secret_key(inner) }
        }),
        Value::Array(items) => items.iter().find_map(secret_key),
        _ => None,
    }
}

fn payload(raw: Option<Value>) -> Result<Option<Value>, String> {
    let Some(value) = raw.filter(|v| !v.is_null()) else { return Ok(None) };
    if !value.is_object() {
        return Err("payload must be a JSON object".into());
    }
    if value.to_string().len() > MAX_PAYLOAD_BYTES {
        return Err(format!("payload must be at most {MAX_PAYLOAD_BYTES} bytes"));
    }
    if let Some(key) = secret_key(&value) {
        return Err(format!("payload must not carry secrets: drop '{key}'"));
    }
    Ok(Some(value))
}

fn timestamp(field: &str, raw: Option<&str>) -> Result<Option<DateTime<Utc>>, String> {
    raw.map(|raw| {
        DateTime::parse_from_rfc3339(raw.trim())
            .map(|at| at.with_timezone(&Utc))
            .map_err(|_| format!("{field} must be an RFC3339 timestamp"))
    })
    .transpose()
}

fn schedule(req: &CreateEvent, now: DateTime<Utc>) -> Result<Window, String> {
    let not_before = timestamp("not_before", req.not_before.as_deref())?;
    let expires_at = timestamp("expires_at", req.expires_at.as_deref())?;
    if not_before.is_some_and(|at| at > now + Duration::days(MAX_HORIZON_DAYS)) {
        return Err(format!("not_before must be at most {MAX_HORIZON_DAYS} days ahead"));
    }
    let earliest = not_before.map_or(now, |at| at.max(now));
    if expires_at.is_some_and(|at| at <= earliest) {
        return Err("expires_at must be after now and after not_before".into());
    }
    Ok((not_before, expires_at))
}

pub fn validate(req: CreateEvent, now: DateTime<Utc>) -> Result<ValidEvent, String> {
    let session_id = target(&req)?;
    let source = Source::parse(req.source.trim()).ok_or_else(|| {
        format!(
            "unknown source '{}': one of n8n, webhook, schedule, timer, outcome, approval, human",
            req.source
        )
    })?;
    let source_id = source_id(source, req.source_id.as_deref())?;
    if req.body.trim().is_empty() {
        return Err("body must not be empty".into());
    }
    if req.body.len() > MAX_BODY_BYTES {
        return Err(format!("body must be at most {MAX_BODY_BYTES} bytes"));
    }
    let (not_before, expires_at) = schedule(&req, now)?;
    Ok(ValidEvent {
        session_id,
        source,
        source_id,
        body: req.body,
        payload: payload(req.payload)?,
        not_before,
        expires_at,
    })
}

pub async fn create(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<CreateEvent>,
) -> ApiResult<(StatusCode, Json<CreateEventResponse>)> {
    let event = validate(req, Utc::now()).map_err(bad_request)?;
    crate::authz::authorize_session_write(&ctx, &event.session_id, &state.pool)
        .await
        .map_err(|code| AppError::new(code, "no session you can write to with that id"))?;
    let owner = store::session_lane_owner(&state.pool, &event.session_id)
        .await?
        .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "no session with that id"))?
        .unwrap_or(ctx.user_id);
    let lane_id = store::session_lane(&state.pool, owner, &event.session_id).await?;
    let inserted = store::insert_event(
        &state.pool,
        &NewEvent {
            lane_id,
            user_id: ctx.user_id,
            source: event.source,
            source_id: event.source_id.as_deref(),
            body: &event.body,
            payload: event.payload.as_ref(),
            not_before: event.not_before,
            expires_at: event.expires_at,
        },
    )
    .await?;
    if !inserted.deduplicated {
        crate::inbox::deliver::wake_lane(&state, lane_id);
    }
    let status = if inserted.deduplicated { "deduplicated" } else { "accepted" };
    Ok((StatusCode::ACCEPTED, Json(CreateEventResponse { id: inserted.id, status: status.into() })))
}

pub async fn list(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Query(query): Query<ListQuery>,
) -> ApiResult<Json<Vec<Event>>> {
    let status = query
        .status
        .as_deref()
        .map(|raw| Status::parse(raw).ok_or_else(|| bad_request(format!("unknown status '{raw}'"))))
        .transpose()?;
    let lane = query.lane.as_deref().filter(|lane| !lane.is_empty());
    Ok(Json(store::list(&state.pool, ctx.owner_filter(), lane, status).await?))
}

pub async fn get_one(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Event>> {
    store::get(&state.pool, ctx.owner_filter(), id).await?.map(Json).ok_or_else(not_found)
}

pub async fn cancel(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    ctx.requires(Scope::Dispatch).map_err(|code| {
        AppError::new(code, "cancelling an inbox event needs the dispatch scope")
    })?;
    match store::cancel(&state.pool, ctx.owner_filter(), id).await? {
        Cancel::Cancelled => Ok(StatusCode::NO_CONTENT),
        Cancel::NotFound => Err(not_found()),
        Cancel::NotPending(status) => Err(AppError::new(
            StatusCode::CONFLICT,
            format!("the event is {status}: only a pending event can be cancelled"),
        )),
    }
}

#[cfg(test)]
#[path = "inbox_tests.rs"]
mod tests;
