use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use cctui_proto::api::SpawnResponse;

use crate::auth::AuthContext;
use crate::error::AppError;
use crate::scheduled_messages::{KIND, claim_one, deliver, parse_deliver_at, seal, settle};
use crate::state::AppState;

type ApiResult<T> = Result<T, AppError>;

/// The queue's state names over the inbox's.
macro_rules! legacy_state {
    () => {
        "CASE e.status WHEN 'pending' THEN 'scheduled' WHEN 'claimed' THEN 'sending' \
              WHEN 'delivered' THEN 'sent' ELSE e.status END"
    };
}

fn not_pending() -> AppError {
    AppError::new(StatusCode::NOT_FOUND, "no pending scheduled message with that id")
}

fn validate_body(body: &str) -> Result<(), &'static str> {
    if body.trim().is_empty() { Err("message must not be empty") } else { Ok(()) }
}

#[derive(Debug, Deserialize)]
pub struct PatchScheduled {
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub deliver_at: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ValidPatch {
    pub body: Option<String>,
    pub deliver_at: Option<DateTime<Utc>>,
}

pub fn validate_patch(patch: PatchScheduled, now: DateTime<Utc>) -> Result<ValidPatch, String> {
    if patch.body.is_none() && patch.deliver_at.is_none() {
        return Err("nothing to update: pass body and/or deliver_at".into());
    }
    if let Some(body) = &patch.body {
        validate_body(body)?;
    }
    let deliver_at = patch
        .deliver_at
        .as_deref()
        .map(|raw| parse_deliver_at(raw, now))
        .transpose()
        .map_err(|e| e.to_string())?;
    Ok(ValidPatch { body: patch.body, deliver_at })
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct ScheduledMessage {
    pub id: uuid::Uuid,
    pub body: String,
    pub state: String,
    pub deliver_at: DateTime<Utc>,
    pub attempts: i32,
    pub last_error: Option<String>,
    pub turn_id: uuid::Uuid,
    pub origin: String,
    pub created_at: DateTime<Utc>,
    pub sent_at: Option<DateTime<Utc>>,
}

pub async fn schedule(
    state: &AppState,
    ctx: &AuthContext,
    session_id: &str,
    content: &str,
    raw_deliver_at: &str,
) -> ApiResult<(StatusCode, Json<SpawnResponse>)> {
    validate_body(content).map_err(|e| AppError::new(StatusCode::BAD_REQUEST, e))?;
    let deliver_at = parse_deliver_at(raw_deliver_at, Utc::now())
        .map_err(|e| AppError::new(StatusCode::BAD_REQUEST, e.to_string()))?;
    let (body, sealed_body) = seal(content, crate::crypto::vault_key)
        .map_err(|e| AppError::new(StatusCode::BAD_REQUEST, e))?;
    let owner = crate::inbox::store::session_lane_owner(&state.pool, session_id)
        .await?
        .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "no session with that id"))?
        .unwrap_or(ctx.user_id);
    let lane_id = crate::inbox::store::session_lane(&state.pool, owner, session_id).await?;
    let id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO inbox_events \
           (lane_id, user_id, source, body, sealed_body, payload, not_before, next_attempt_at, \
            batch_id) \
         VALUES ($1, $2, 'human', $3, $4, $5, $6, $6, gen_random_uuid()) RETURNING id",
    )
    .bind(lane_id)
    .bind(ctx.user_id)
    .bind(body)
    .bind(sealed_body)
    .bind(serde_json::json!({ "kind": KIND, "origin": "human" }))
    .bind(deliver_at)
    .fetch_one(&state.pool)
    .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(SpawnResponse {
            command_id: id,
            status: "scheduled".into(),
            account: None,
            session_id: None,
        }),
    ))
}

/// Pending, dead-lettered, and the last day of delivered messages.
pub async fn list(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> ApiResult<Json<Vec<ScheduledMessage>>> {
    let rows = sqlx::query_as(concat!(
        "SELECT e.id, e.body, ",
        legacy_state!(),
        " AS state, e.not_before AS deliver_at, e.attempts, e.last_error, \
                COALESCE(e.batch_id, e.id) AS turn_id, \
                COALESCE(e.payload->>'origin', 'human') AS origin, e.created_at, \
                e.delivered_at AS sent_at \
         FROM inbox_events e JOIN inbox_lanes l ON l.id = e.lane_id \
         WHERE l.key = 'session:' || $1::text AND e.payload->>'kind' = 'scheduled_message' \
           AND (e.status IN ('pending', 'claimed', 'dead') \
                OR (e.status = 'delivered' AND e.delivered_at > now() - interval '1 day')) \
         ORDER BY e.not_before"
    ))
    .bind(&session_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

pub async fn update(
    State(state): State<AppState>,
    Path((session_id, queue_id)): Path<(String, uuid::Uuid)>,
    Json(patch): Json<PatchScheduled>,
) -> ApiResult<StatusCode> {
    let patch =
        validate_patch(patch, Utc::now()).map_err(|e| AppError::new(StatusCode::BAD_REQUEST, e))?;
    let (body, sealed_body) = match patch.body.as_deref() {
        None => (None, None),
        Some(body) => {
            let (body, sealed) = seal(body, crate::crypto::vault_key)
                .map_err(|e| AppError::new(StatusCode::BAD_REQUEST, e))?;
            (Some(body), sealed)
        }
    };
    let res = sqlx::query(
        "UPDATE inbox_events e SET \
           body = COALESCE($3, e.body), \
           sealed_body = CASE WHEN $3::text IS NULL THEN e.sealed_body ELSE $5 END, \
           not_before = COALESCE($4, e.not_before), \
           next_attempt_at = COALESCE($4, e.next_attempt_at), \
           attempts = CASE WHEN $4 IS NULL THEN e.attempts ELSE 0 END \
         FROM inbox_lanes l \
         WHERE l.id = e.lane_id AND e.id = $1 AND l.key = 'session:' || $2::text \
           AND e.status = 'pending' AND e.payload->>'kind' = 'scheduled_message'",
    )
    .bind(queue_id)
    .bind(&session_id)
    .bind(body)
    .bind(patch.deliver_at)
    .bind(sealed_body)
    .execute(&state.pool)
    .await?;
    if res.rows_affected() == 0 {
        return Err(not_pending());
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn cancel(
    State(state): State<AppState>,
    Path((session_id, queue_id)): Path<(String, uuid::Uuid)>,
) -> ApiResult<StatusCode> {
    let res = sqlx::query(
        "UPDATE inbox_events e SET status = 'cancelled' \
         FROM inbox_lanes l \
         WHERE l.id = e.lane_id AND e.id = $1 AND l.key = 'session:' || $2::text \
           AND e.status IN ('pending', 'dead') AND e.payload->>'kind' = 'scheduled_message'",
    )
    .bind(queue_id)
    .bind(&session_id)
    .execute(&state.pool)
    .await?;
    if res.rows_affected() == 0 {
        return Err(not_pending());
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn send_now(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path((session_id, queue_id)): Path<(String, uuid::Uuid)>,
) -> ApiResult<StatusCode> {
    crate::routes::sessions::authorize_reply(&state, &ctx, &session_id).await?;
    let row = claim_one(&state.pool, &session_id, queue_id).await?.ok_or_else(not_pending)?;
    let (turn_id, attempts) = (row.turn_id, row.attempts);
    let outcome = deliver(&state, row).await;
    settle(&state.pool, turn_id, attempts, outcome)
        .await?
        .map_err(|e| AppError::new(StatusCode::SERVICE_UNAVAILABLE, e))?;
    Ok(StatusCode::ACCEPTED)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-09-24T10:00:00Z").unwrap().with_timezone(&Utc)
    }

    fn patch(body: Option<&str>, deliver_at: Option<&str>) -> PatchScheduled {
        PatchScheduled { body: body.map(Into::into), deliver_at: deliver_at.map(Into::into) }
    }

    #[test]
    fn patch_needs_a_field() {
        assert!(validate_patch(patch(None, None), now()).is_err());
    }

    #[test]
    fn patch_rejects_blank_body_and_bad_times() {
        assert!(validate_patch(patch(Some("  "), None), now()).is_err());
        assert!(validate_patch(patch(None, Some("2026-09-24T09:00:00Z")), now()).is_err());
        assert!(validate_patch(patch(None, Some("2026-11-01T00:00:00Z")), now()).is_err());
        assert!(validate_patch(patch(None, Some("tomorrow")), now()).is_err());
    }

    #[test]
    fn patch_accepts_body_and_reschedule() {
        let ok = validate_patch(patch(Some("new"), Some("2026-09-25T09:00:00Z")), now()).unwrap();
        assert_eq!(ok.body.as_deref(), Some("new"));
        assert_eq!(ok.deliver_at.map(|d| d.to_rfc3339()), Some("2026-09-25T09:00:00+00:00".into()));
    }

    #[test]
    fn a_tagged_body_is_stored_masked_and_sealed() {
        let key = vec![7u8; 32];
        let content = "deploy with <redacted>hunter2-xyz</redacted>";
        let (body, sealed) = seal(content, || key.clone()).unwrap();
        assert_eq!(body, "deploy with [REDACTED:user]");
        let sealed = sealed.expect("the real text is kept sealed");
        assert!(!sealed.contains("hunter2-xyz"));
        assert_eq!(crate::crypto::decrypt(&sealed, &key).as_deref(), Some(content));
    }

    #[test]
    fn an_untagged_body_is_stored_as_is_without_reading_the_key() {
        let (body, sealed) = seal("plain", || panic!("no span, no key read")).unwrap();
        assert_eq!((body.as_str(), sealed), ("plain", None));
    }

    #[test]
    fn a_tagged_body_is_refused_without_a_vault_key() {
        assert_eq!(
            seal("<redacted>hunter2-xyz</redacted>", Vec::new),
            Err(crate::scheduled_messages::SEAL_NEEDS_KEY)
        );
    }

    #[test]
    fn schedule_body_must_not_be_blank() {
        assert!(validate_body("").is_err());
        assert!(validate_body("\n\t").is_err());
        assert!(validate_body("ping").is_ok());
    }
}
