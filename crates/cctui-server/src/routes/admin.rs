use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use cctui_proto::api::{ApiError, MessageRequest, SessionListItem, SessionListResponse};

use crate::state::AppState;

#[derive(sqlx::FromRow)]
struct DbSession {
    id: Uuid,
    parent_id: Option<Uuid>,
    machine_id: String,
    working_dir: String,
    status: String,
    registered_at: DateTime<Utc>,
    metadata: serde_json::Value,
}

pub async fn list_sessions(
    State(state): State<AppState>,
) -> Result<Json<SessionListResponse>, (StatusCode, Json<ApiError>)> {
    // Live sessions from in-memory registry
    let mut sessions: Vec<SessionListItem> = {
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

    // Historical sessions from DB (terminated/disconnected, not in registry)
    let live_ids: Vec<Uuid> = sessions.iter().map(|s| s.id).collect();
    let rows: Vec<DbSession> = sqlx::query_as(
        "SELECT id, parent_id, machine_id, working_dir, status, registered_at, metadata \
         FROM sessions WHERE status IN ('terminated', 'disconnected') \
         ORDER BY registered_at DESC LIMIT 50",
    )
    .fetch_all(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("db error: {e}");
        (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "database error".into() }))
    })?;

    for row in rows {
        if live_ids.contains(&row.id) {
            continue;
        }
        let status = if row.status == "disconnected" {
            cctui_proto::models::SessionStatus::Disconnected
        } else {
            cctui_proto::models::SessionStatus::Terminated
        };
        sessions.push(SessionListItem {
            id: row.id,
            parent_id: row.parent_id,
            machine_id: row.machine_id,
            working_dir: row.working_dir,
            status,
            uptime_secs: (Utc::now() - row.registered_at).num_seconds(),
            token_usage: cctui_proto::models::TokenUsage::default(),
            metadata: row.metadata,
        });
    }

    Ok(Json(SessionListResponse { sessions }))
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
        "SELECT payload FROM stream_events WHERE session_id = $1 ORDER BY created_at ASC",
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
