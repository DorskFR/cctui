use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::Utc;

use cctui_proto::api::{ApiError, RegisterRequest, RegisterResponse};
use cctui_proto::models::{Session, SessionStatus};

use crate::state::AppState;

pub async fn register(
    State(state): State<AppState>,
    Json(req): Json<RegisterRequest>,
) -> Result<Json<RegisterResponse>, (StatusCode, Json<ApiError>)> {
    // Use Claude's session_id directly — it's our primary key now
    let session_id = req.claude_session_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let now = Utc::now();
    let session = Session {
        id: session_id.clone(),
        parent_id: req.parent_session_id,
        account_id: None,
        machine_id: req.machine_id,
        working_dir: req.working_dir,
        status: SessionStatus::Active,
        registered_at: now,
        last_heartbeat: now,
        metadata: req.metadata.unwrap_or_else(|| serde_json::json!({})),
    };

    sqlx::query(
        r"INSERT INTO sessions (id, parent_id, account_id, machine_id, working_dir, status, registered_at, last_heartbeat, metadata)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
           ON CONFLICT (id) DO UPDATE SET status = 'active', last_heartbeat = $8, metadata = $9",
    )
    .bind(&session.id)
    .bind(&session.parent_id)
    .bind(&session.account_id)
    .bind(&session.machine_id)
    .bind(&session.working_dir)
    .bind("active")
    .bind(session.registered_at)
    .bind(session.last_heartbeat)
    .bind(&session.metadata)
    .execute(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("db error: {e}");
        (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "database error".into() }))
    })?;

    let ws_url = format!(
        "{}/api/v1/stream/{}",
        state.config.external_url.replacen("http://", "ws://", 1).replacen("https://", "wss://", 1),
        session_id
    );
    {
        let mut registry = state.registry.write().await;
        registry.register(session.clone());
    }

    tracing::info!(session_id = %session_id, machine = %session.machine_id, "session registered");
    Ok(Json(RegisterResponse { session_id, ws_url }))
}

pub async fn deregister(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    sqlx::query("UPDATE sessions SET status = 'terminated' WHERE id = $1")
        .bind(&session_id)
        .execute(&state.pool)
        .await
        .map_err(|e| {
            tracing::error!("db error: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "database error".into() }))
        })?;
    {
        let mut registry = state.registry.write().await;
        registry.deregister(&session_id);
    }
    tracing::info!(session_id = %session_id, "session deregistered");
    Ok(StatusCode::NO_CONTENT)
}
