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
    let launch = core::machine_launches::find(&host.pool, &session, ctx.owner_filter()).await?;
    let canonical = launch.as_ref().and_then(|launch| launch.registered_session_id.clone());
    let session = canonical.unwrap_or_else(|| {
        launch.as_ref().map_or_else(|| session.clone(), |launch| launch.session_id.clone())
    });
    let owner = launch.as_ref().map(|launch| launch.user_id).or_else(|| ctx.owner_filter());
    let machine = launch.as_ref().map(|launch| launch.machine_id);
    let row:Option<core::results::ResultRow>=sqlx::query_as("SELECT adapter_id,end_reason,ended_at,status,tempo,agent_state,activity,soft_limit_reason FROM sessions WHERE id=$1 AND ($2::uuid IS NULL OR user_id=$2) AND ($3::uuid IS NULL OR machine_uuid=$3)").bind(&session).bind(owner).bind(machine).fetch_optional(&host.pool).await?;
    let Some(row) = row else {
        if let Some(stop) = crate::termination::find(&host, &session).await?
            && owner.is_none_or(|owner| owner == stop.user_id)
            && machine.is_none_or(|machine| stop.machine_id == Some(machine))
        {
            return Ok(Json(
                serde_json::json!({"done":true,"end_reason":stop.end_reason,"final_assistant_text":null,"message_id":null,"token_usage":null,"queued":false,"queue_position":null,"final_assistant_text_truncated":false}),
            ));
        }

        if let Some(launch) = launch {
            return Ok(Json(
                serde_json::json!({"done":launch.failed(),"end_reason":if launch.failed(){Some("spawn_failed")}else{None},"final_assistant_text":null,"message_id":null,"token_usage":null,"queued":false,"queue_position":null,"final_assistant_text_truncated":false}),
            ));
        }
        let result = core::results::unregistered_result(&host.pool, &session, ctx.owner_filter())
            .await?
            .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "session not found"))?;
        let mut value = serde_json::to_value(result).expect("run result is serializable");
        value["final_assistant_text_truncated"] = serde_json::json!(false);
        return Ok(Json(value));
    };
    let value:Option<serde_json::Value>=sqlx::query_scalar("SELECT metadata->'cctui_execution_result' FROM sessions WHERE id=$1 AND ($2::uuid IS NULL OR user_id=$2) AND ($3::uuid IS NULL OR machine_uuid=$3)").bind(&session).bind(owner).bind(machine).fetch_one(&host.pool).await?;
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
    if cancel_dispatch(&host, &ctx, &session).await? {
        return Ok(StatusCode::NO_CONTENT);
    }
    let guard = host.lifecycle.lock().await;
    let launch = core::machine_launches::find(&host.pool, &session, ctx.owner_filter()).await?;
    let canonical =
        launch.as_ref().and_then(|launch| launch.registered_session_id.clone()).unwrap_or_else(
            || launch.as_ref().map_or_else(|| session.clone(), |launch| launch.session_id.clone()),
        );
    let row:Option<(uuid::Uuid,uuid::Uuid,String)>=sqlx::query_as("SELECT machine_uuid,user_id,adapter_id FROM sessions WHERE id=$1 AND machine_uuid IS NOT NULL AND user_id IS NOT NULL AND adapter_id IS NOT NULL AND ($2::uuid IS NULL OR user_id=$2)")
        .bind(&canonical).bind(launch.as_ref().map(|launch|launch.user_id).or_else(||ctx.owner_filter())).fetch_optional(&host.pool).await?;
    let (machine, user, adapter) = if let Some(launch) = &launch {
        (launch.machine_id, launch.user_id, launch.adapter_id.clone())
    } else {
        row.ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "session not found"))?
    };
    let mut identities =
        vec![launch.as_ref().map_or_else(|| canonical.clone(), |launch| launch.session_id.clone())];
    if canonical != identities[0] {
        identities.push(canonical.clone());
    }
    crate::termination::record(
        &host,
        &identities,
        user,
        Some(machine),
        &adapter,
        "killed",
        "cancelled by API",
    )
    .await?;
    drop(guard);
    crate::termination::stop_worker(&host, machine, &canonical, &adapter).await;
    Ok(StatusCode::NO_CONTENT)
}

/// Backend I/O stays outside the lifecycle lock. Once the backend acknowledges,
/// refresh registration under the lock before committing the terminal decision.
async fn cancel_dispatch(host: &Host, ctx: &AuthContext, session: &str) -> Result<bool, AppError> {
    let row:Option<(uuid::Uuid,String,Option<String>)>=sqlx::query_as("SELECT r.user_id,r.dispatcher_name,h.handle FROM dispatch_records r LEFT JOIN dispatch_handles h USING (session_id) WHERE r.session_id=$1 AND r.user_id IS NOT NULL AND ($2::uuid IS NULL OR r.user_id=$2)")
        .bind(session).bind(ctx.owner_filter()).fetch_optional(&host.pool).await?;
    let Some((user, name, handle)) = row else { return Ok(false) };
    {
        let _guard = host.lifecycle.lock().await;
        if let Some(stop) = crate::termination::find(host, session).await?
            && stop.user_id == user
        {
            return Ok(true);
        }
    }
    let handle = handle.ok_or_else(|| {
        AppError::new(
            StatusCode::CONFLICT,
            "dispatch delivery is unresolved; no cancellation handle is available",
        )
    })?;
    let backend = core::dispatch::resolve_dispatcher(host, Some(user), &name)
        .await
        .map_err(|error| dispatch_error(&error))?
        .ok_or_else(|| AppError::new(StatusCode::SERVICE_UNAVAILABLE, "dispatcher unavailable"))?;
    backend.cancel(&handle).await.map_err(|error| dispatch_error(&error))?;
    let guard = host.lifecycle.lock().await;
    let row: Option<(Option<uuid::Uuid>, Option<String>)> =
        sqlx::query_as("SELECT machine_uuid,adapter_id FROM sessions WHERE id=$1 AND user_id=$2")
            .bind(session)
            .bind(user)
            .fetch_optional(&host.pool)
            .await?;
    let (machine, adapter) = row.unwrap_or_default();
    crate::termination::record(
        host,
        &[session.to_owned()],
        user,
        machine,
        adapter.as_deref().unwrap_or(""),
        "killed",
        "cancelled by API",
    )
    .await?;
    drop(guard);
    if let (Some(machine), Some(adapter)) = (machine, adapter) {
        crate::termination::stop_worker(host, machine, session, &adapter).await;
    }
    Ok(true)
}
fn dispatch_error(error: &core::dispatchers::DispatchError) -> AppError {
    let status = if matches!(error, core::dispatchers::DispatchError::Unsupported(_)) {
        StatusCode::NOT_IMPLEMENTED
    } else {
        StatusCode::BAD_GATEWAY
    };
    AppError::new(status, error.to_string())
}
