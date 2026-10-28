//! Owned run result and cancellation endpoints.
use crate::host::Host;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use cctui_server_core as core;
use core::auth::{AuthContext, Scope};
use core::error::AppError;

pub async fn result(
    State(host): State<Host>,
    Extension(ctx): Extension<AuthContext>,
    Path(session): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    ctx.requires(Scope::Read).map_err(|status| AppError::new(status, "read scope required"))?;
    let row:Option<core::results::ResultRow>=sqlx::query_as("SELECT adapter_id,end_reason,ended_at,status,tempo,agent_state,activity,soft_limit_reason FROM sessions WHERE id=$1 AND ($2::uuid IS NULL OR user_id=$2)").bind(&session).bind(ctx.owner_filter()).fetch_optional(&host.pool).await?;
    let Some(row) = row else {
        let result = core::results::unregistered_result(&host.pool, &session, ctx.owner_filter())
            .await?
            .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "session not found"))?;
        let mut value = serde_json::to_value(result).expect("run result is serializable");
        value["final_assistant_text_truncated"] = serde_json::json!(false);
        return Ok(Json(value));
    };
    let value:Option<serde_json::Value>=sqlx::query_scalar("SELECT metadata->'cctui_execution_result' FROM sessions WHERE id=$1 AND ($2::uuid IS NULL OR user_id=$2)").bind(&session).bind(ctx.owner_filter()).fetch_one(&host.pool).await?;
    let capture: core::results::CapturedResult =
        value.and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
    let response = cctui_proto::api::SessionResult {
        done: row.done(),
        end_reason: row.end_reason,
        final_assistant_text: capture.text(),
        message_id: capture.message_id.clone(),
        token_usage: None,
        queued: false,
        queue_position: None,
    };
    let mut response = serde_json::to_value(response).expect("run result is serializable");
    response["final_assistant_text_truncated"] = serde_json::json!(capture.truncated);
    Ok(Json(response))
}

pub async fn kill(
    State(host): State<Host>,
    Extension(ctx): Extension<AuthContext>,
    Path(session): Path<String>,
) -> Result<StatusCode, AppError> {
    ctx.requires(Scope::Dispatch)
        .map_err(|status| AppError::new(status, "dispatch scope required"))?;
    let row:Option<(uuid::Uuid,uuid::Uuid,String)>=sqlx::query_as("SELECT machine_uuid,user_id,adapter_id FROM sessions WHERE id=$1 AND machine_uuid IS NOT NULL AND user_id IS NOT NULL AND adapter_id IS NOT NULL AND ($2::uuid IS NULL OR user_id=$2)")
        .bind(&session).bind(ctx.owner_filter()).fetch_optional(&host.pool).await?;
    let (machine, user, adapter) =
        row.ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "session not found"))?;
    core::lifecycle::persist_session_end(
        &host.pool,
        &core::lifecycle::NoopSessionEndJournal,
        machine,
        user,
        &session,
        &cctui_proto::adapter::EndReason::Killed,
    )
    .await?;
    core::session_credentials::revoke_session_tokens(&host, &session).await;
    if let Err(error) = host
        .bus
        .command_daemon_for_session(
            machine,
            &session,
            cctui_proto::ws::DaemonFrameDown::Command {
                adapter_id: adapter,
                command: Box::new(cctui_proto::adapter::AdapterCommand::Kill {
                    local_id: session.clone(),
                    signal: None,
                }),
            },
        )
        .await
    {
        tracing::warn!(%error,%session,"cancellation persisted; worker stop could not be delivered");
    }
    Ok(StatusCode::NO_CONTENT)
}
