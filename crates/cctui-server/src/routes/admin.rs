use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};

use crate::registry::PendingMessage;
use cctui_proto::api::{ApiError, MessageRequest, SessionListItem, SessionListResponse};

use crate::state::AppState;

#[derive(sqlx::FromRow)]
struct DbSession {
    id: String,
    parent_id: Option<String>,
    machine_id: String,
    working_dir: String,
    status: String,
    registered_at: DateTime<Utc>,
    metadata: serde_json::Value,
}

pub async fn list_sessions(
    State(state): State<AppState>,
) -> Result<Json<SessionListResponse>, (StatusCode, Json<ApiError>)> {
    // Live sessions from in-memory registry — keep registered_at for sorting.
    let mut with_ts: Vec<(DateTime<Utc>, SessionListItem)> = {
        let registry = state.registry.read().await;
        registry
            .list()
            .into_iter()
            .map(|handle| {
                (
                    handle.session.registered_at,
                    SessionListItem {
                        id: handle.session.id.clone(),
                        parent_id: handle.session.parent_id.clone(),
                        machine_id: handle.session.machine_id.clone(),
                        working_dir: handle.session.working_dir.clone(),
                        status: handle.session.status,
                        uptime_secs: (Utc::now() - handle.session.registered_at).num_seconds(),
                        token_usage: handle.token_usage.clone(),
                        metadata: handle.session.metadata.clone(),
                    },
                )
            })
            .collect()
    };

    // Historical inactive sessions from DB (not currently in the live registry).
    let live_ids: Vec<String> = with_ts.iter().map(|(_, s)| s.id.clone()).collect();
    let rows: Vec<DbSession> = sqlx::query_as(
        "SELECT id, parent_id, machine_id, working_dir, status, registered_at, metadata \
         FROM sessions WHERE status = 'inactive' \
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
        let status = match row.status.as_str() {
            "new" => cctui_proto::models::SessionStatus::New,
            "active" => cctui_proto::models::SessionStatus::Active,
            _ => cctui_proto::models::SessionStatus::Inactive,
        };
        with_ts.push((
            row.registered_at,
            SessionListItem {
                id: row.id,
                parent_id: row.parent_id,
                machine_id: row.machine_id,
                working_dir: row.working_dir,
                status,
                uptime_secs: (Utc::now() - row.registered_at).num_seconds(),
                token_usage: cctui_proto::models::TokenUsage::default(),
                metadata: row.metadata,
            },
        ));
    }

    with_ts.sort_by(|a, b| b.0.cmp(&a.0));
    let sessions = with_ts.into_iter().map(|(_, s)| s).collect();
    Ok(Json(SessionListResponse { sessions }))
}

pub async fn get_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<SessionListItem>, (StatusCode, Json<ApiError>)> {
    let registry = state.registry.read().await;
    let handle = registry.get(&session_id).ok_or_else(|| {
        (StatusCode::NOT_FOUND, Json(ApiError { error: "session not found".into() }))
    })?;
    let item = SessionListItem {
        id: handle.session.id.clone(),
        parent_id: handle.session.parent_id.clone(),
        machine_id: handle.session.machine_id.clone(),
        working_dir: handle.session.working_dir.clone(),
        status: handle.session.status,
        uptime_secs: (Utc::now() - handle.session.registered_at).num_seconds(),
        token_usage: handle.token_usage.clone(),
        metadata: handle.session.metadata.clone(),
    };
    drop(registry);
    Ok(Json(item))
}

pub async fn get_conversation(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<Vec<serde_json::Value>>, (StatusCode, Json<ApiError>)> {
    let rows: Vec<(serde_json::Value,)> = sqlx::query_as(
        "SELECT payload FROM stream_events WHERE session_id = $1 ORDER BY created_at ASC",
    )
    .bind(&session_id)
    .fetch_all(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("db error: {e}");
        (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "database error".into() }))
    })?;
    Ok(Json(rows.into_iter().map(|(v,)| v).collect()))
}

pub async fn send_message(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(req): Json<MessageRequest>,
) -> StatusCode {
    let mut registry = state.registry.write().await;
    registry.queue_message(&session_id, req.content);
    StatusCode::ACCEPTED
}

pub async fn get_pending_messages(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Json<Vec<PendingMessage>> {
    let messages = state.registry.write().await.take_pending_messages(&session_id);
    Json(messages)
}

pub async fn kill_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    // Kill drops the in-memory handle and marks the DB row inactive. The
    // session isn't archived — activity on the channel can revive it.
    sqlx::query("UPDATE sessions SET status = 'inactive' WHERE id = $1")
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
    tracing::info!(session_id = %session_id, "session killed");
    Ok(StatusCode::NO_CONTENT)
}

pub async fn set_session_policy(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(rules): Json<Vec<crate::policy::PolicyRule>>,
) -> StatusCode {
    let mut registry = state.registry.write().await;
    registry.set_policy(&session_id, rules);
    StatusCode::OK
}
