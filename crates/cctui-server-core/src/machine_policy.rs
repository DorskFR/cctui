//! Machine launch validation, authorization and durable child grants shared by both hosts.
use crate::auth::AuthContext;
use crate::error::AppError;
use crate::launch_credentials::LaunchCredentialsContext;
use crate::resource_owners::{Shareable, shareable_owner};
use crate::state::CoreState;
use axum::Json;
use axum::http::StatusCode;
use cctui_proto::api::{ApiError, SpawnRequest};
use uuid::Uuid;
fn bad_request(message: impl Into<String>) -> (StatusCode, Json<ApiError>) {
    (StatusCode::BAD_REQUEST, Json(ApiError { error: message.into() }))
}
pub fn require_dispatch(ctx: &AuthContext) -> Result<(), (StatusCode, Json<ApiError>)> {
    ctx.requires(crate::auth::Scope::Dispatch).map_err(|code| {
        (code, Json(ApiError { error: "spawning needs the dispatch scope".into() }))
    })
}
pub fn validate_env(req: &SpawnRequest) -> Result<(), (StatusCode, Json<ApiError>)> {
    for key in req.env.keys() {
        let ok = !key.is_empty()
            && key.bytes().next().is_some_and(|b| b == b'_' || b.is_ascii_uppercase())
            && key.bytes().all(|b| b == b'_' || b.is_ascii_uppercase() || b.is_ascii_digit());
        if !ok {
            return Err(bad_request(format!("invalid env key {key:?} (want ^[A-Z_][A-Z0-9_]*$)")));
        }
    }
    Ok(())
}
pub fn spawn_adapter(requested: Option<&str>) -> Result<String, String> {
    let id = requested.map(str::trim).filter(|s| !s.is_empty()).unwrap_or("claude-code");
    if crate::gateway::Family::try_from_adapter(id).is_none() {
        return Err(format!("unknown adapter {id:?}"));
    }
    Ok(id.to_owned())
}

/// The requested adapter, refused when the machine reports its binary
/// missing or a dropped privacy grant.
pub async fn spawn_adapter_on(
    state: &CoreState,
    machine_uuid: Uuid,
    requested: Option<&str>,
) -> Result<String, (StatusCode, Json<ApiError>)> {
    let adapter_id = spawn_adapter(requested).map_err(bad_request)?;
    let (machine, report) = crate::machine_adapters::machine_report(&state.pool, machine_uuid)
        .await
        .map_err(AppError::into_parts)?;
    crate::machine_adapters::spawn_refusal(report.as_ref(), &machine, &adapter_id)
        .map_or(Ok(adapter_id), |error| Err((StatusCode::CONFLICT, Json(ApiError { error }))))
}

pub async fn default_capability_for(
    state: &impl LaunchCredentialsContext,
    machine_uuid: Uuid,
) -> cctui_proto::api::SpawnCapability {
    let cap = state.default_spawn_capability().await;
    match crate::targets::daemon::reconcile::enabled_adapter_ids(state.launch_state(), machine_uuid)
        .await
    {
        Ok(ids) => cap.with_adapters(spawnable_adapters(ids)),
        Err(e) => {
            tracing::warn!(%machine_uuid, error = %e, "enabled adapters unknown; granting the defaults");
            cap
        }
    }
}

/// Only the adapters the server can launch: an enabled ACP agent is not a
/// family `spawn_child` knows, so advertising it promises a child that always
/// fails with "unknown harness".
#[must_use]
pub fn spawnable_adapters(ids: Vec<String>) -> Vec<String> {
    ids.into_iter().filter(|id| crate::gateway::Family::try_from_adapter(id).is_some()).collect()
}

/// Keyed by the id the worker will register as, and stored before dispatch so
/// the capability resolves the moment the worker asks.
pub async fn persist_spawn_capability(
    state: &impl LaunchCredentialsContext,
    machine_uuid: Uuid,
    req: &SpawnRequest,
    permission_mode: Option<cctui_proto::adapter::PermissionMode>,
    token_session_id: &str,
) {
    let mut cap = if let Some(cap) = req.spawn_capability.clone().filter(|c| !c.is_empty()) {
        cap
    } else {
        default_capability_for(state, machine_uuid).await
    };
    let launched = permission_mode.unwrap_or(cctui_proto::adapter::PermissionMode::Ask);
    cap.max_permission_mode = Some(
        cap.max_permission_mode
            .map_or(launched, |c| cctui_proto::adapter::PermissionMode::stricter(c, launched)),
    );
    if let Err(e) =
        crate::store::spawn_capabilities::upsert(&state.launch_state().pool, token_session_id, &cap)
            .await
    {
        tracing::error!(
            session = %token_session_id,
            error = %e,
            "spawn-capability persist failed — CctuiAgent will be lost on server restart"
        );
    }
    state.launch_state().spawn_capabilities.insert(token_session_id.to_owned(), cap);
}

pub async fn machine_and_owner(
    state: &CoreState,
    machine_id: &str,
) -> Result<(Uuid, Uuid), (StatusCode, Json<ApiError>)> {
    let machine_uuid =
        Uuid::parse_str(machine_id).map_err(|_| bad_request("machine_id must be a uuid"))?;
    let owner = shareable_owner(Shareable::Machine, machine_uuid, &state.pool)
        .await
        .map_err(|e| AppError::from(e).into_parts())?;
    let Some(owner) = owner else {
        return Err((StatusCode::NOT_FOUND, Json(ApiError { error: "machine not found".into() })));
    };
    Ok((machine_uuid, owner))
}

fn not_your_machine() -> (StatusCode, Json<ApiError>) {
    (StatusCode::FORBIDDEN, Json(ApiError { error: "not your machine".into() }))
}

/// Resolve `machine_id` (a UUID) to `(machine_uuid, owner)`, enforcing
/// `admin || caller == owner`.
pub async fn resolve_owned_machine(
    state: &CoreState,
    ctx: &AuthContext,
    machine_id: &str,
) -> Result<(Uuid, Uuid), (StatusCode, Json<ApiError>)> {
    let (machine_uuid, owner) = machine_and_owner(state, machine_id).await?;
    if !(ctx.is_admin() || ctx.user_id == owner) {
        return Err(not_your_machine());
    }
    Ok((machine_uuid, owner))
}

/// The share action that lets a non-owner spawn on a machine.
const SPAWN_SHARE_ACTION: &str = "spawn";

/// [`resolve_owned_machine`], also admitting a user holding a live `spawn`
/// share on the machine. A plain `use` share does not admit. The session stays
/// the machine owner's either way.
pub async fn resolve_spawn_machine(
    state: &CoreState,
    ctx: &AuthContext,
    machine_id: &str,
) -> Result<(Uuid, Uuid), (StatusCode, Json<ApiError>)> {
    let (machine_uuid, owner) = machine_and_owner(state, machine_id).await?;
    may_spawn_on(state, ctx, machine_uuid, owner).await?;
    Ok((machine_uuid, owner))
}

/// `admin || caller == owner || caller holds a live spawn share` on the machine.
pub async fn may_spawn_on(
    state: &CoreState,
    ctx: &AuthContext,
    machine_uuid: Uuid,
    owner: Uuid,
) -> Result<(), (StatusCode, Json<ApiError>)> {
    if ctx.is_admin() || ctx.user_id == owner {
        return Ok(());
    }
    let shared = crate::store::shares::granted_action(
        &state.pool,
        Shareable::Machine.as_share_type(),
        machine_uuid,
        ctx.user_id,
        SPAWN_SHARE_ACTION,
    )
    .await
    .map_err(|e| AppError::from(e).into_parts())?;
    if shared { Ok(()) } else { Err(not_your_machine()) }
}

/// The gate every path that starts a worker on `machine_uuid` applies: the
/// `dispatch` scope, and the machine authorization a spawn needs.
pub async fn authorize_launch(
    state: &CoreState,
    ctx: &AuthContext,
    machine_uuid: Uuid,
) -> Result<(), AppError> {
    launch_gate(state, ctx, machine_uuid)
        .await
        .map_err(|(code, Json(e))| AppError::new(code, e.error))
}

async fn launch_gate(
    state: &CoreState,
    ctx: &AuthContext,
    machine_uuid: Uuid,
) -> Result<(), (StatusCode, Json<ApiError>)> {
    require_dispatch(ctx)?;
    let (_, owner) = machine_and_owner(state, &machine_uuid.to_string()).await?;
    may_spawn_on(state, ctx, machine_uuid, owner).await
}

pub async fn fill_spawner(pool: &sqlx::PgPool, session_id: &str, user_id: Uuid) {
    let res = sqlx::query(
        "UPDATE sessions SET spawned_by = $2 \
         WHERE id = $1 AND spawned_by IS NULL",
    )
    .bind(session_id)
    .bind(user_id)
    .execute(pool)
    .await;
    if let Err(e) = res {
        tracing::warn!(%session_id, error = %e, "could not record the spawner");
    }
}

/// Remember who asked for the spawn under `spawn_key`, and the name they
/// gave it, and drop day-old intents nobody claimed. Best-effort: a lost
/// intent leaves `spawned_by` unset and the name unpinned.
pub async fn remember_spawner(
    pool: &sqlx::PgPool,
    spawn_key: &str,
    user_id: Uuid,
    name: Option<&str>,
) {
    let name = name.map(str::trim).filter(|n| !n.is_empty());
    let res = async {
        sqlx::query(
            "DELETE FROM session_spawner_intents WHERE created_at < now() - interval '1 day'",
        )
        .execute(pool)
        .await?;
        sqlx::query(
            "INSERT INTO session_spawner_intents (spawn_key, user_id, session_name) \
             VALUES ($1, $2, $3) \
             ON CONFLICT (spawn_key) DO UPDATE SET user_id = EXCLUDED.user_id, \
                session_name = EXCLUDED.session_name, created_at = now()",
        )
        .bind(spawn_key)
        .bind(user_id)
        .bind(name)
        .execute(pool)
        .await
    }
    .await;
    if let Err(e) = res {
        tracing::warn!(%spawn_key, error = %e, "could not record the spawner");
    }
}

/// Stamp `sessions.spawned_by` from a pending intent, looked up by the
/// session's own id and by the spawn key the adapter echoed.
///
/// A name given at
/// spawn becomes the session name, pinned against agent titles. The intent is
/// only consumed once the row exists, or an early claim would lose the name.
pub async fn claim_spawner(pool: &sqlx::PgPool, session_id: &str, spawn_key: Option<&str>) {
    let key = spawn_key.unwrap_or(session_id);
    let res = sqlx::query(
        "WITH claimed AS (DELETE FROM session_spawner_intents \
                           WHERE (spawn_key = $1 OR spawn_key = $2) \
                             AND EXISTS (SELECT 1 FROM sessions WHERE id = $1) \
                           RETURNING user_id, session_name), \
              named AS (SELECT session_name FROM claimed \
                        WHERE session_name IS NOT NULL LIMIT 1) \
         UPDATE sessions SET spawned_by = (SELECT user_id FROM claimed LIMIT 1), \
            session_name = COALESCE((SELECT session_name FROM named), sessions.session_name), \
            metadata = CASE WHEN EXISTS (SELECT 1 FROM named) \
                THEN COALESCE(sessions.metadata, '{}'::jsonb) \
                     || jsonb_build_object('name_source', 'user') \
                ELSE sessions.metadata END \
         WHERE id = $1 AND EXISTS (SELECT 1 FROM claimed)",
    )
    .bind(session_id)
    .bind(key)
    .execute(pool)
    .await;
    if let Err(e) = res {
        tracing::warn!(%session_id, error = %e, "could not record the spawner");
    }
}
