use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::Utc;
use uuid::Uuid;

use cctui_proto::api::{ApiError, MessageRequest, SessionListItem, SessionListResponse};

use crate::state::AppState;

pub async fn list_sessions(State(state): State<AppState>) -> Json<SessionListResponse> {
    let sessions = {
        let registry = state.registry.read().await;
        registry
            .list()
            .into_iter()
            .map(|handle| SessionListItem {
                id: handle.session.id,
                parent_id: handle.session.parent_id,
                machine_id: handle.session.machine_id.clone(),
                working_dir: handle.session.working_dir.clone(),
                status: handle.session.status.clone(),
                uptime_secs: (Utc::now() - handle.session.registered_at).num_seconds(),
                token_usage: handle.token_usage.clone(),
                metadata: handle.session.metadata.clone(),
            })
            .collect()
    };
    Json(SessionListResponse { sessions })
}

pub async fn get_session(
    State(state): State<AppState>,
    Path(session_id): Path<Uuid>,
) -> Result<Json<SessionListItem>, (StatusCode, Json<ApiError>)> {
    let registry = state.registry.read().await;
    let handle = registry.get(&session_id).ok_or_else(|| {
        (StatusCode::NOT_FOUND, Json(ApiError { error: "session not found".into() }))
    })?;
    let item = SessionListItem {
        id: handle.session.id,
        parent_id: handle.session.parent_id,
        machine_id: handle.session.machine_id.clone(),
        working_dir: handle.session.working_dir.clone(),
        status: handle.session.status.clone(),
        uptime_secs: (Utc::now() - handle.session.registered_at).num_seconds(),
        token_usage: handle.token_usage.clone(),
        metadata: handle.session.metadata.clone(),
    };
    drop(registry);
    Ok(Json(item))
}

pub async fn get_conversation(
    State(state): State<AppState>,
    Path(session_id): Path<Uuid>,
) -> Result<Json<Vec<serde_json::Value>>, (StatusCode, Json<ApiError>)> {
    let rows: Vec<(serde_json::Value,)> = sqlx::query_as(
        "SELECT event_data FROM stream_events WHERE session_id = $1 ORDER BY created_at ASC",
    )
    .bind(session_id)
    .fetch_all(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("db error: {e}");
        (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "database error".into() }))
    })?;
    Ok(Json(rows.into_iter().map(|(v,)| v).collect()))
}

pub async fn send_message(
    State(_state): State<AppState>,
    Path(_session_id): Path<Uuid>,
    Json(_req): Json<MessageRequest>,
) -> StatusCode {
    StatusCode::ACCEPTED
}

pub async fn kill_session(
    State(state): State<AppState>,
    Path(session_id): Path<Uuid>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    sqlx::query("UPDATE sessions SET status = 'terminated' WHERE id = $1")
        .bind(session_id)
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
    tracing::info!(session_id = %session_id, "session killed");
    Ok(StatusCode::NO_CONTENT)
}
