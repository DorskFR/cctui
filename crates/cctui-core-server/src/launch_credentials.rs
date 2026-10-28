//! Daemon launch pulls assembled from shared ownership, minting and grant services.
use crate::host::Host;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use cctui_server_core as core;
use core::launch_credentials::{gateway_env_allowed, spawn_capability_for};

async fn machine(host: &Host, headers: &HeaderMap) -> Result<(uuid::Uuid, uuid::Uuid), StatusCode> {
    let token =
        core::targets::daemon::transport::bearer_token(headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let ctx = host.auth_config.validate(&token).await.ok_or(StatusCode::UNAUTHORIZED)?;
    Ok((ctx.user_id, ctx.machine_id.ok_or(StatusCode::FORBIDDEN)?))
}

pub async fn gateway_env(
    State(host): State<Host>,
    headers: HeaderMap,
    Path(session): Path<String>,
) -> Result<Json<cctui_proto::api::GatewayEnvResponse>, StatusCode> {
    let (user, machine) = machine(&host, &headers).await?;
    let allowed = gateway_env_allowed(&host.pool, user, machine, &session)
        .await
        .map_err(|error| db_error(&error))?;
    let mut reply = cctui_proto::api::GatewayEnvResponse {
        account_bound: false,
        env: std::collections::BTreeMap::default(),
        settings: None,
        whip_phrases: None,
        spawn_capability: None,
        plugins: Vec::new(),
        context: Vec::new(),
    };
    if !allowed {
        return Ok(Json(reply));
    }
    reply.spawn_capability = spawn_capability_for(&host, &session).await;
    let accounts = core::gateway::mint::resolve_session_accounts(&host, &session).await;
    if accounts.is_empty() {
        return Ok(Json(reply));
    }
    reply.account_bound = true;
    for account in accounts {
        if let Some(env) =
            core::gateway::mint::mint_session_env_for_account(&host, account, &session)
                .await
                .map_err(|error| db_error(&error))?
        {
            reply.env.extend(env);
        }
    }
    reply.settings = core::gateway::mint::resolve_session_settings(&host, &session).await;
    let adapter = core::store::sessions::adapter_id(&host.pool, &session)
        .await
        .map_err(|error| db_error(&error))?
        .unwrap_or_default();
    if core::execution_settings::adapter_takes_service_tier(&adapter) {
        reply.settings = core::execution_settings::overlay_service_tier(reply.settings);
    }
    // Match the full host: a bound worker pulling credentials is relaunching.
    sqlx::query("UPDATE sessions SET status='active',ended_at=NULL,end_reason=NULL,end_detail=NULL WHERE id=$1 AND status='ended'").bind(&session).execute(&host.pool).await.map_err(|error| db_error(&error))?;
    Ok(Json(reply))
}

pub async fn context(
    State(host): State<Host>,
    headers: HeaderMap,
    Path(session): Path<String>,
) -> Result<Json<cctui_proto::api::LaunchContextResponse>, StatusCode> {
    let (user, machine) = machine(&host, &headers).await?;
    gateway_env_allowed(&host.pool, user, machine, &session)
        .await
        .map_err(|error| db_error(&error))?;
    Ok(Json(cctui_proto::api::LaunchContextResponse { context: Vec::new() }))
}

#[derive(serde::Deserialize)]
pub struct TokenQuery {
    hash: String,
}

pub async fn token_valid(
    State(host): State<Host>,
    headers: HeaderMap,
    Path(session): Path<String>,
    axum::extract::Query(query): axum::extract::Query<TokenQuery>,
) -> Result<Json<cctui_proto::api::TokenValidResponse>, StatusCode> {
    let (user, _) = machine(&host, &headers).await?;
    let valid = core::launch_credentials::token_valid(&host.pool, user, &session, &query.hash)
        .await
        .map_err(|error| db_error(&error))?
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(cctui_proto::api::TokenValidResponse { valid }))
}
fn db_error(error: &sqlx::Error) -> StatusCode {
    tracing::error!(%error,"daemon launch credential lookup failed");
    StatusCode::INTERNAL_SERVER_ERROR
}
