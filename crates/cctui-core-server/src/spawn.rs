//! JSON machine launches for the automation distribution.
use crate::host::Host;
use axum::extract::State;
use axum::http::StatusCode;
use axum::{Extension, Json};
use cctui_proto::adapter::{AdapterId, SessionSpec};
use cctui_proto::api::{SpawnRequest, SpawnResponse};
use cctui_server_core as core;
use core::auth::AuthContext;
use core::error::AppError;
use uuid::Uuid;

pub async fn spawn(
    State(host): State<Host>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<SpawnRequest>,
) -> Result<(StatusCode, Json<SpawnResponse>), AppError> {
    execute(&host, &ctx, &req).await.map_err(|(code, Json(error))| AppError::new(code, error.error))
}

type Failure = (StatusCode, Json<cctui_proto::api::ApiError>);
fn unsupported(req: &SpawnRequest) -> Result<(), Failure> {
    let unsupported = req.save_draft
        || req.auto_archive
        || !req.env_keys.is_empty()
        || !req.attachment_names.is_empty()
        || !req.label_ids.is_empty()
        || req.context.is_some()
        || req.profile_id.is_some()
        || req.prompt_name.is_some()
        || req.relation.is_some()
        || req.parent_session_id.is_some();
    if unsupported {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(cctui_proto::api::ApiError {
                error: "workspace spawn fields are unavailable in the automation distribution"
                    .into(),
            }),
        ));
    }
    Ok(())
}
async fn execute(
    host: &Host,
    ctx: &AuthContext,
    req: &SpawnRequest,
) -> Result<(StatusCode, Json<SpawnResponse>), Failure> {
    core::machine_policy::require_dispatch(ctx)?;
    unsupported(req)?;
    core::machine_policy::validate_env(req)?;
    let (machine, owner) =
        core::machine_policy::resolve_spawn_machine(host, ctx, &req.machine_id).await?;
    // Registration currently binds credentials to the machine owner. Refuse a
    // cross-user grant until execution records can preserve the acting user.
    if !ctx.is_admin() && ctx.user_id != owner {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(cctui_proto::api::ApiError {
                error: "cross-user machine launches are not yet available in the automation beta"
                    .into(),
            }),
        ));
    }
    let adapter =
        core::machine_policy::spawn_adapter_on(host, machine, req.adapter_id.as_deref()).await?;
    let command_id = Uuid::new_v4();
    let (session_id, token_key) = core::launch::spawn_ids(&adapter, command_id);
    let token_session = token_key.to_string();
    let bound = core::machine_spawn::resolve_spawn_account(
        host,
        &core::machine_spawn::AccountTarget {
            uid: ctx.owner_filter().unwrap_or(owner),
            adapter_id: adapter.clone(),
            token_session_id: token_session.clone(),
        },
        req,
    )
    .await?;
    core::machine_policy::remember_spawner(
        &host.pool,
        &token_session,
        ctx.user_id,
        req.name.as_deref(),
    )
    .await;
    let service_tier = if core::settings_catalog::codex::adapter_takes_service_tier(&adapter) {
        let settings = core::gateway::mint::resolve_session_settings(host, &token_session).await;
        Some(core::settings_catalog::codex::resolve_service_tier(
            req.service_tier.as_deref(),
            settings.as_ref(),
        ))
    } else {
        None
    };
    core::machine_policy::persist_spawn_capability(
        host,
        machine,
        req,
        bound.permission_mode,
        &token_session,
    )
    .await;
    let failed = core::commands::FailedSpawnRow {
        session_id: token_session.clone(),
        machine_id: machine,
        user_id: owner,
        adapter_id: adapter.clone(),
        working_dir: req.working_dir.clone(),
        name: req.name.clone().filter(|n| !n.trim().is_empty()),
        model: bound.model.clone(),
        effort: bound.effort.clone(),
    };
    let spec = SessionSpec {
        adapter_id: AdapterId::new(&adapter),
        working_dir: Some(req.working_dir.clone()),
        prompt: req.prompt.clone().filter(|p| !p.trim().is_empty()),
        name: req.name.clone(),
        permission_mode: bound.permission_mode,
        effort: bound.effort,
        model: bound.model,
        service_tier,
        env: bound.env,
        bootstrap: serde_json::Value::Null,
        parent_local_id: None,
    };
    core::launch::launch_durable(
        &host.core,
        ctx.user_id,
        core::launch::MachineSpawn { machine_id: machine, command_id, token_key, spec, failed },
    )
    .await
    .map_err(launch_error)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(SpawnResponse {
            command_id,
            status: "dispatched".into(),
            account: bound.account_choice,
            session_id,
        }),
    ))
}

fn launch_error(error: core::launch::LaunchError) -> Failure {
    let detail = match error {
        core::launch::LaunchError::Storage(error) => {
            tracing::error!(%error,"could not persist launch");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(cctui_proto::api::ApiError { error: core::error::DB_ERROR.into() }),
            );
        }
        core::launch::LaunchError::Refused(detail) => {
            format!("the daemon refused the spawn: {detail}")
        }
        core::launch::LaunchError::Delivery(core::bus::BusError::NoDaemon(_)) => {
            "daemon for that machine is offline — start `cctui-daemon` first".into()
        }
        core::launch::LaunchError::Delivery(_) => "daemon disconnected mid-dispatch".into(),
    };
    (StatusCode::SERVICE_UNAVAILABLE, Json(cctui_proto::api::ApiError { error: detail }))
}
