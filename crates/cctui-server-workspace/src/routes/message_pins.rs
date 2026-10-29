//! Per-user message pins inside a session transcript.
//!
//! Rows are scoped to the calling user, so two users pinning the same session
//! keep separate collections; session-read/write authz comes from the
//! `api_router` layer.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use cctui_proto::models::MessagePin;
use chrono::{DateTime, Utc};

use crate::auth::AuthContext;
use crate::error::AppError;
use crate::state::AppState;

type PinRow = (String, i64, Option<String>, Option<String>, DateTime<Utc>);

fn to_pin(row: PinRow) -> MessagePin {
    MessagePin { session_id: row.0, seq: row.1, message_id: row.2, note: row.3, created_at: row.4 }
}

pub async fn list_pins(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(session_id): Path<String>,
) -> Result<Json<Vec<MessagePin>>, AppError> {
    let rows: Vec<PinRow> = sqlx::query_as(
        "SELECT session_id, seq, message_id, note, created_at \
         FROM session_message_pins WHERE user_id = $1 AND session_id = $2 \
         ORDER BY seq ASC",
    )
    .bind(ctx.user_id)
    .bind(&session_id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows.into_iter().map(to_pin).collect()))
}

#[derive(Debug, serde::Deserialize)]
pub struct CreatePin {
    pub seq: i64,
    #[serde(default)]
    pub message_id: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

pub async fn create_pin(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(session_id): Path<String>,
    Json(body): Json<CreatePin>,
) -> Result<Json<MessagePin>, AppError> {
    if body.seq <= 0 {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "seq must be positive"));
    }
    let row: PinRow = sqlx::query_as(
        "INSERT INTO session_message_pins (user_id, session_id, seq, message_id, note) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (user_id, session_id, seq) DO UPDATE \
           SET message_id = COALESCE(EXCLUDED.message_id, session_message_pins.message_id), \
               note = COALESCE(EXCLUDED.note, session_message_pins.note) \
         RETURNING session_id, seq, message_id, note, created_at",
    )
    .bind(ctx.user_id)
    .bind(&session_id)
    .bind(body.seq)
    .bind(body.message_id.as_deref())
    .bind(body.note.as_deref())
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(to_pin(row)))
}

pub async fn delete_pin(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path((session_id, seq)): Path<(String, i64)>,
) -> Result<StatusCode, AppError> {
    sqlx::query(
        "DELETE FROM session_message_pins WHERE user_id = $1 AND session_id = $2 AND seq = $3",
    )
    .bind(ctx.user_id)
    .bind(&session_id)
    .bind(seq)
    .execute(&state.pool)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
