use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::Source;
use super::store::{self, Lineage, NewEvent};
use crate::context::SessionsContext;
use crate::error::AppError;

pub const MIN_DELAY_SECS: u32 = 60;
const MAX_PROMPT_BYTES: usize = 16 * 1024;
const MAX_TOOL_USE_ID_CHARS: usize = 128;
/// Lets the tool result reach the agent before its session is archived.
const ARCHIVE_GRACE: std::time::Duration = std::time::Duration::from_secs(10);

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimerRequest {
    pub delay_secs: u32,
    pub prompt: String,
    pub tool_use_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Tick,
    Event,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum Accepted {
    Tick { fires_at: DateTime<Utc> },
    Event { fires_at: DateTime<Utc>, event_id: Uuid, deduplicated: bool, archives: bool },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TimerResult {
    pub error: Option<String>,
    pub timer: Option<Accepted>,
}

fn max_delay_secs() -> u32 {
    u32::try_from(crate::scheduled_messages::MAX_HORIZON_DAYS * 86_400).unwrap_or(u32::MAX)
}

pub fn validate(req: &TimerRequest) -> Result<(), String> {
    let max = max_delay_secs();
    if !(MIN_DELAY_SECS..=max).contains(&req.delay_secs) {
        return Err(format!("delay_secs must be between {MIN_DELAY_SECS} and {max}"));
    }
    if req.prompt.trim().is_empty() {
        return Err("prompt must not be empty: it is the message you wake up to".into());
    }
    if req.prompt.len() > MAX_PROMPT_BYTES {
        return Err(format!("prompt must be at most {MAX_PROMPT_BYTES} bytes"));
    }
    let id = req.tool_use_id.trim();
    if id.is_empty() || id.chars().count() > MAX_TOOL_USE_ID_CHARS {
        return Err(format!("tool_use_id must be 1-{MAX_TOOL_USE_ID_CHARS} characters"));
    }
    Ok(())
}

#[must_use]
pub fn mode_for(delay_secs: u32, adapter_id: Option<&str>, model: Option<&str>) -> Mode {
    if delay_secs <= crate::keepalive::default_interval_secs(adapter_id, model) {
        Mode::Tick
    } else {
        Mode::Event
    }
}

#[derive(sqlx::FromRow)]
struct SessionFacts {
    adapter_id: Option<String>,
    model: Option<String>,
    pinned: bool,
    owner: Option<Uuid>,
}

async fn facts(pool: &sqlx::PgPool, session_id: &str) -> sqlx::Result<Option<SessionFacts>> {
    sqlx::query_as(
        "SELECT s.adapter_id, s.model, s.pinned, COALESCE(m.user_id, s.user_id) AS owner \
         FROM sessions s LEFT JOIN machines m ON m.id = s.machine_uuid WHERE s.id = $1",
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await
}

async fn lane_of(pool: &sqlx::PgPool, owner: Uuid, session_id: &str) -> sqlx::Result<Uuid> {
    let preset_lane: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM inbox_lanes WHERE user_id = $1 AND session_id = $2 \
         ORDER BY preset_id IS NULL, created_at DESC LIMIT 1",
    )
    .bind(owner)
    .bind(session_id)
    .fetch_optional(pool)
    .await?;
    if let Some(id) = preset_lane {
        return Ok(id);
    }
    store::session_lane(pool, owner, session_id).await
}

async fn queue_event(
    pool: &sqlx::PgPool,
    owner: Uuid,
    session_id: &str,
    req: &TimerRequest,
    fires_at: DateTime<Utc>,
) -> Result<store::Inserted, AppError> {
    let lane_id = lane_of(pool, owner, session_id).await?;
    let source_id = format!("{session_id}:{}", req.tool_use_id.trim());
    let event = NewEvent {
        lane_id,
        user_id: owner,
        source: Source::Timer,
        source_id: Some(&source_id),
        body: req.prompt.trim(),
        payload: None,
        not_before: Some(fires_at),
        expires_at: None,
    };
    let lineage = Lineage { hops: 0, parent_session_id: Some(session_id) };
    store::insert_traced(pool, &event, lineage)
        .await?
        .ok_or_else(|| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "timer event refused"))
}

/// The inner `Err` is a refusal the agent reads.
///
/// # Errors
/// A database failure.
pub async fn schedule<S: SessionsContext>(
    state: &S,
    session_id: &str,
    req: &TimerRequest,
    now: DateTime<Utc>,
) -> Result<Result<Accepted, String>, AppError> {
    if let Err(e) = validate(req) {
        return Ok(Err(e));
    }
    let Some(session) = facts(state.pool(), session_id).await? else {
        return Err(AppError::new(StatusCode::NOT_FOUND, "calling session not found"));
    };
    let fires_at = now + Duration::seconds(i64::from(req.delay_secs));
    if mode_for(req.delay_secs, session.adapter_id.as_deref(), session.model.as_deref())
        == Mode::Tick
    {
        let prompt = req.prompt.trim();
        if !crate::keepalive::arm_timer(state.pool(), session_id, fires_at, prompt).await? {
            return Ok(Err(format!(
                "timer budget spent: {} wakes without a human message",
                crate::keepalive::DEFAULT_MAX_TICKS
            )));
        }
        return Ok(Ok(Accepted::Tick { fires_at }));
    }
    let Some(owner) = session.owner else {
        return Ok(Err("a session without an owner has no inbox lane".into()));
    };
    let inserted = queue_event(state.pool(), owner, session_id, req, fires_at).await?;
    Ok(Ok(Accepted::Event {
        fires_at,
        event_id: inserted.id,
        deduplicated: inserted.deduplicated,
        archives: !session.pinned && !inserted.deduplicated,
    }))
}

pub async fn archive<S: SessionsContext>(state: &S, session_id: &str) -> sqlx::Result<bool> {
    let outcome = crate::routes::sessions::archive_one(
        state,
        session_id,
        false,
        cctui_proto::adapter::RemoveInitiator::Automatic,
        crate::events::Actor::Agent(session_id.to_owned()),
    )
    .await?;
    Ok(outcome != crate::routes::sessions::ArchiveOutcome::SkippedPinned)
}

fn archive_later<S: SessionsContext>(state: &S, session_id: &str) {
    let (state, session_id) = (state.clone(), session_id.to_owned());
    tokio::spawn(async move {
        tokio::time::sleep(ARCHIVE_GRACE).await;
        if let Err(e) = archive(&state, &session_id).await {
            tracing::warn!(%session_id, "archiving after a long timer failed: {e}");
        }
    });
}

/// `POST /api/v1/daemon/sessions/{id}/timer`.
///
/// # Errors
/// The caller does not own the session, or a database failure.
pub async fn daemon_timer<S: SessionsContext>(
    State(state): State<S>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(req): Json<TimerRequest>,
) -> Result<Json<TimerResult>, AppError> {
    crate::outcome::daemon_session(&state, &headers, &session_id).await?;
    let result = match schedule(&state, &session_id, &req, Utc::now()).await? {
        Ok(accepted) => {
            if matches!(accepted, Accepted::Event { archives: true, .. }) {
                archive_later(&state, &session_id);
            }
            TimerResult { error: None, timer: Some(accepted) }
        }
        Err(error) => TimerResult { error: Some(error), timer: None },
    };
    Ok(Json(result))
}

#[cfg(test)]
mod tests {

    use super::{Mode, TimerRequest, mode_for, validate};

    fn request(delay_secs: u32) -> TimerRequest {
        TimerRequest {
            delay_secs,
            prompt: " check the CI run ".into(),
            tool_use_id: "toolu_1".into(),
        }
    }

    #[test]
    fn a_timer_needs_a_delay_in_range_a_prompt_and_a_tool_use_id() {
        assert_eq!(validate(&request(600)), Ok(()));
        assert!(validate(&request(59)).unwrap_err().contains("delay_secs"));
        assert!(validate(&request(u32::MAX)).unwrap_err().contains("delay_secs"));
        let blank = TimerRequest { prompt: "\n".into(), ..request(600) };
        assert!(validate(&blank).unwrap_err().contains("prompt"));
        let no_id = TimerRequest { tool_use_id: " ".into(), ..request(600) };
        assert!(validate(&no_id).unwrap_err().contains("tool_use_id"));
    }

    #[test]
    fn the_cache_ttl_minus_its_margin_splits_ticks_from_events() {
        assert_eq!(mode_for(600, Some("claude-code"), None), Mode::Tick);
        assert_eq!(mode_for(3480, Some("claude-code"), None), Mode::Tick);
        assert_eq!(mode_for(3481, Some("claude-code"), None), Mode::Event);
        assert_eq!(mode_for(180, Some("codex"), Some("gpt-5.5")), Mode::Tick);
        assert_eq!(mode_for(181, Some("codex"), Some("gpt-5.5")), Mode::Event);
    }
}
