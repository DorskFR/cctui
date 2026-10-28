//! `POST /api/v1/sessions/spawn`.
//!
//! Pushes an `AdapterCommand::Spawn` to the targeted daemon over the
//! existing WS command channel. The daemon's adapter resolves the spawn
//! against its underlying agent (claude-code dispatches via the
//! `claude daemon` control socket; codex parity follows in).
//!
//! Failure modes:
//!   * Daemon offline → 503 with hint.
//!   * Caller without the `dispatch` scope → 403.
//!   * Machine neither owned by the caller nor shared to them with `spawn` → 403.
//!   * Unknown machine → 404.
//!
//! Mapping the returned `command_id` to the eventual `session_id` is the
//! client's job: it watches `/sessions` (or the TUI WS) for a new live
//! session and matches on `(machine_id, working_dir, registered_at >=
//! request_time)`. A future iteration can plumb the daemon's spawn ACK
//! back through the WS for an explicit mapping.

use axum::extract::{Multipart, Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};

use cctui_proto::adapter::{AdapterId, BootstrapUploads, SessionSpec};
use cctui_proto::api::{
    ApiError, LaunchRequest, ScheduleLaunchRequest, SpawnRequest, SpawnResponse,
};
use uuid::Uuid;

use cctui_server_core::launch::spawn_ids;

use crate::auth::AuthContext;
use crate::error::AppError;
use crate::registry::MachineCommand;
use crate::routes::server_settings::cached_upload_caps;
use crate::state::AppState;
use crate::store::sessions::SessionRowStatus;
use crate::uploads::parse_upload_multipart;

/// `POST /api/v1/sessions/spawn` — `multipart/form-data`.
///
/// Parts:
///   * `request` — the JSON [`SpawnRequest`] (machine, cwd, prompt, env, …).
///   * any part with a `filename` — a file to stage for the worker.
///
/// Files are base64-encoded into `SessionSpec.bootstrap` (the WS leg is JSON);
/// the daemon decodes + writes them to `/tmp/cctui-uploads/<session-id>/` and
/// references their paths in the prompt. `env` secrets ride on `SessionSpec.env`
/// (never persisted/logged) and the daemon injects them into the worker process.
pub async fn spawn_session(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    multipart: Multipart,
) -> Result<(StatusCode, Json<SpawnResponse>), AppError> {
    let parsed = parse_upload_multipart(multipart, cached_upload_caps(&state))
        .await
        .map_err(|(code, Json(e))| AppError::new(code, e.error))?;
    let uploads = parsed.files;
    let req: SpawnRequest = parsed
        .request_json
        .ok_or_else(|| AppError::new(StatusCode::BAD_REQUEST, "missing `request` part"))
        .and_then(|raw| {
            serde_json::from_str(&raw).map_err(|e| {
                AppError::new(StatusCode::BAD_REQUEST, format!("invalid SpawnRequest JSON: {e}"))
            })
        })?;

    // Draft: stage the spawn payload as a `draft` session row and stop
    // — no env minted, no daemon dispatch, no model turn. Launched later via
    // `POST /sessions/{id}/launch`, which re-reads the files from the blob store.
    if req.save_draft {
        return save_draft(&state, &ctx, &req, &parsed.raw).await;
    }

    dispatch_spawn(&state, &ctx, req, uploads, parsed.raw)
        .await
        .map_err(|(code, Json(e))| AppError::new(code, e.error))
}

/// Dispatch a spawn to the targeted daemon. Shared by the immediate spawn path
/// and the draft-launch path so account env is minted + the command
/// dispatched identically. Validates the `dispatch` scope, env keys and machine
/// access, mints any account gateway env, and pushes `AdapterCommand::Spawn`
/// over the WS.
pub async fn dispatch_spawn(
    state: &AppState,
    ctx: &AuthContext,
    req: SpawnRequest,
    uploads: Vec<cctui_proto::adapter::BootstrapFile>,
    raw_uploads: Vec<crate::uploads::RawUpload>,
) -> Result<(StatusCode, Json<SpawnResponse>), (StatusCode, Json<ApiError>)> {
    require_dispatch(ctx)?;
    let target = validate_spawn(state, ctx, &req).await?;
    let bound = resolve_spawn_account(state, &target, &req).await?;
    execute_spawn(state, target, bound, &req, uploads, raw_uploads).await
}

/// Starting a run needs the `dispatch` scope: a read-only key may watch, not launch.
fn require_dispatch(ctx: &AuthContext) -> Result<(), (StatusCode, Json<ApiError>)> {
    ctx.requires(crate::auth::Scope::Dispatch).map_err(|code| {
        (code, Json(ApiError { error: "spawning needs the dispatch scope".into() }))
    })
}

use cctui_server_core::machine_spawn::BoundAccount;
pub(super) use cctui_server_core::machine_spawn::auto_account_name_scoped;
#[cfg(test)]
use cctui_server_core::machine_spawn::{
    AccountDecision, AutoAccountRow, auto_account_candidates, decide_account,
    resolve_default_account,
};

async fn resolve_spawn_account(
    state: &AppState,
    target: &SpawnTarget,
    req: &SpawnRequest,
) -> Result<BoundAccount, (StatusCode, Json<ApiError>)> {
    cctui_server_core::machine_spawn::resolve_spawn_account(
        state,
        &cctui_server_core::machine_spawn::AccountTarget {
            uid: target.uid,
            adapter_id: target.adapter_id.clone(),
            token_session_id: target.token_session_id.clone(),
        },
        req,
    )
    .await
}

/// The validated spawn target plus the ids minted for it.
struct SpawnTarget {
    machine_uuid: Uuid,
    owner: Uuid,
    /// The user whose accounts the spawn resolves against.
    uid: Uuid,
    adapter_id: String,
    command_id: Uuid,
    /// For claude-code the session id is pre-minted here and handed to the
    /// worker as `--session-id` (mirroring the fork path), so the gateway token
    /// can be bound to the *real* session id the worker registers as. Only
    /// claude registers under it, so only claude gets it back in the response.
    pre_session_id: Option<Uuid>,
    /// The key the gateway session token, spawn capability and spawn intents
    /// are stored under: the pre-minted session id for claude, else the
    /// `command_id`. Always sent to the daemon as `Spawn.session_id`, whose
    /// adapters echo it as `spawn_key` on `SessionStarted` so the server can
    /// re-key all of it onto the id the harness really registered under.
    token_key: Uuid,
    /// `token_key` as the text id the stores use.
    token_session_id: String,
    /// The context items this spawn resolved, server-side, from the request's
    /// picks, the profile's pinned set and the scope-matching memories.
    context: Vec<crate::routes::context::ContextItem>,
}

use cctui_server_core::machine_policy::{resolve_spawn_machine, spawn_adapter_on};
#[cfg(test)]
use cctui_server_core::machine_policy::{spawn_adapter, spawnable_adapters};

async fn validate_spawn(
    state: &AppState,
    ctx: &AuthContext,
    req: &SpawnRequest,
) -> Result<SpawnTarget, (StatusCode, Json<ApiError>)> {
    cctui_server_core::machine_policy::validate_env(req)?;

    let (machine_uuid, owner) = resolve_spawn_machine(state, ctx, &req.machine_id).await?;

    let adapter_id = spawn_adapter_on(state, machine_uuid, req.adapter_id.as_deref()).await?;
    let command_id = Uuid::new_v4();
    let (pre_session_id, token_key) = spawn_ids(&adapter_id, command_id);
    let token_session_id = token_key.to_string();
    if req.auto_archive {
        crate::auto_archive::remember_intent(state, &token_session_id).await;
    }
    remember_spawner(&state.pool, &token_session_id, ctx.user_id, req.name.as_deref()).await;
    // In the machine owner's vocabulary: an admin's picker holds labels of
    // every user, and the session belongs to the owner.
    let label_ids =
        crate::spawn_labels::for_spawn(&state.pool, &req.label_ids, ctx.owner_filter(), owner)
            .await;
    crate::spawn_labels::remember_intent(&state.pool, &token_session_id, &label_ids).await;
    crate::followup::remember_intent(&state.pool, &token_session_id, req).await;
    let (context, scope) =
        resolve_spawn_context(state, ctx, req, &label_ids, owner, machine_uuid, &adapter_id).await;
    crate::routes::context::remember_intent(&state.pool, &token_session_id, &context, &scope).await;
    // Accounts are user-owned. The admin token has no user identity, so it
    // resolves the account against the target machine's owner —
    // the session runs on that user's machine with that user's account.
    let uid = ctx.owner_filter().unwrap_or(owner);

    Ok(SpawnTarget {
        machine_uuid,
        owner,
        uid,
        adapter_id,
        command_id,
        pre_session_id,
        token_key,
        token_session_id,
        context,
    })
}

/// Resolve the reusable context for this spawn: the matching rules, the
/// request's explicit picks, the profile's pinned items and — when the request
/// asks for `auto` — every enabled memory whose filter matches the spawn's
/// machine, working dir, labels and adapter. Returns the coordinates too, so
/// every later launch re-resolves the rules against them.
///
/// Best-effort: a lookup failure launches the session without context rather
/// than refusing it. Context is a convenience, never a precondition.
async fn resolve_spawn_context(
    state: &AppState,
    ctx: &AuthContext,
    req: &SpawnRequest,
    label_ids: &[String],
    owner: Uuid,
    machine_uuid: Uuid,
    adapter_id: &str,
) -> (Vec<crate::routes::context::ContextItem>, crate::routes::context::StoredScope) {
    use crate::routes::context::{SpawnScope, StoredScope, resolve_for_spawn};

    let uid = ctx.owner_filter().unwrap_or(owner);
    // The resolved uuid, not `req.machine_id`, which may be a name: a
    // machine filter must resolve the same here and on the child path.
    let scope = StoredScope {
        user_id: uid,
        coords: SpawnScope {
            machine_id: Some(machine_uuid.to_string()),
            working_dir: Some(req.working_dir.clone()),
            label_ids: label_ids.to_vec(),
            adapter: Some(adapter_id.to_owned()),
        },
        dispatched: false,
    };
    let asked = req.context.clone().unwrap_or_default();
    let items = match crate::routes::context::list_for_user(&state.pool, uid).await {
        Ok(items) => items,
        Err(e) => {
            tracing::warn!(error = %e, "context lookup failed; spawning without it");
            return (Vec::new(), scope);
        }
    };
    let mut picks = asked.items;
    picks.extend(profile_context_names(state, req, uid, &items).await);
    (resolve_for_spawn(&items, &picks, &scope.coords), scope)
}

/// The names of the items the request's profile pins. Empty when the request
/// names no profile — the webui also applies a profile by filling the form,
/// and doing both is harmless because picks are unioned by id.
async fn profile_context_names(
    state: &AppState,
    req: &SpawnRequest,
    uid: Uuid,
    items: &[crate::routes::context::ContextItem],
) -> Vec<String> {
    let Some(profile_id) = req.profile_id else { return Vec::new() };
    let pinned: Option<Vec<Uuid>> = sqlx::query_scalar(
        "SELECT context_items FROM session_profiles WHERE id = $1 AND user_id = $2",
    )
    .bind(profile_id)
    .bind(uid)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or_default();
    pinned
        .unwrap_or_default()
        .iter()
        .filter_map(|id| items.iter().find(|i| &i.id == id))
        .map(|i| i.name.clone())
        .collect()
}

async fn stage_uploads(
    state: &AppState,
    token_session_id: &str,
    uploads: Vec<cctui_proto::adapter::BootstrapFile>,
    raw_uploads: Vec<crate::uploads::RawUpload>,
) -> Result<(serde_json::Value, Vec<Uuid>), (StatusCode, Json<ApiError>)> {
    let bootstrap = if uploads.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::to_value(BootstrapUploads { uploads }).map_err(|e| {
            tracing::error!("serializing bootstrap uploads: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError { error: "could not encode uploads".into() }),
            )
        })?
    };
    // Must precede dispatch: nothing is staged yet, so a blob-store failure can
    // still fail the request instead of producing a session whose first message
    // references files the conversation can never re-read. Staged names are the
    // sanitized upload names — the daemon stages into a fresh per-session dir.
    let recorded = if raw_uploads.is_empty() {
        Vec::new()
    } else {
        crate::routes::attachments::record_uploads(&state.pool, token_session_id, &raw_uploads, &[])
            .await
            .map_err(|e| {
                tracing::error!(session = %token_session_id, "recording bootstrap uploads: {e}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiError { error: "could not store the attachments".into() }),
                )
            })?
    };
    Ok((bootstrap, recorded.iter().map(|a| a.id).collect()))
}

async fn spawn_service_tier(
    state: &AppState,
    adapter_id: &str,
    req: &SpawnRequest,
    token_session_id: &str,
) -> Option<String> {
    // Codex's own default tier is `priority`, so an unset tier is the expensive
    // one: resolve to a concrete value here rather than letting the worker
    // inherit whatever the machine's config.toml happens to say.
    if crate::settings_catalog::codex::adapter_takes_service_tier(adapter_id) {
        let account_settings =
            crate::routes::gateway::resolve_session_settings(state, token_session_id).await;
        Some(crate::settings_catalog::codex::resolve_service_tier(
            req.service_tier.as_deref(),
            account_settings.as_ref(),
        ))
    } else {
        None
    }
}

/// The first turn this spawn sends. A selected prompt template replaces the
/// typed prompt — a template IS the prompt, so staging it as a file the agent
/// must go and read would be a pointless indirection — and the typed text
/// rides underneath it as the concrete task.
fn spawn_prompt(
    req: &SpawnRequest,
    context: &[crate::routes::context::ContextItem],
) -> Option<String> {
    let typed = req.prompt.clone().filter(|p| !p.trim().is_empty());
    let Some(template) = context.iter().find(|i| i.kind == "prompt") else {
        return typed;
    };
    let expanded = crate::routes::context::expand_template(
        &template.body,
        &req.working_dir,
        req.name.as_deref().unwrap_or_default(),
        typed.as_deref().unwrap_or_default(),
    );
    match typed {
        Some(typed) if !expanded.contains(&typed) => Some(format!("{expanded}\n\n{typed}")),
        _ => Some(expanded),
    }
}

async fn execute_spawn(
    state: &AppState,
    target: SpawnTarget,
    bound: BoundAccount,
    req: &SpawnRequest,
    uploads: Vec<cctui_proto::adapter::BootstrapFile>,
    raw_uploads: Vec<crate::uploads::RawUpload>,
) -> Result<(StatusCode, Json<SpawnResponse>), (StatusCode, Json<ApiError>)> {
    let SpawnTarget {
        machine_uuid,
        owner,
        adapter_id,
        command_id,
        pre_session_id,
        token_key,
        token_session_id,
        context,
        ..
    } = target;
    let BoundAccount { env, model, effort, permission_mode, account_choice } = bound;
    let (bootstrap, recorded_ids) =
        stage_uploads(state, &token_session_id, uploads, raw_uploads).await?;

    let service_tier = spawn_service_tier(state, &adapter_id, req, &token_session_id).await;
    let spec_model = model.clone();
    let spec_effort = effort.clone();
    let spec = SessionSpec {
        adapter_id: AdapterId::new(&adapter_id),
        working_dir: Some(req.working_dir.clone()),
        prompt: spawn_prompt(req, &context),
        name: req.name.clone(),
        permission_mode,
        effort,
        model,
        service_tier,
        env,
        bootstrap,
        parent_local_id: None,
    };
    persist_spawn_capability(state, machine_uuid, req, permission_mode, &token_session_id).await;
    let launched = cctui_server_core::launch::launch_machine(
        &state.bus,
        &state.pending_commands,
        &state.command_waiters,
        cctui_server_core::launch::MachineSpawn {
            machine_id: machine_uuid,
            command_id,
            token_key,
            spec,
            failed: crate::state::FailedSpawnRow {
                session_id: token_session_id.clone(),
                machine_id: machine_uuid,
                user_id: owner,
                adapter_id: adapter_id.clone(),
                working_dir: req.working_dir.clone(),
                name: req.name.clone().filter(|n| !n.trim().is_empty()),
                model: spec_model,
                effort: spec_effort,
            },
        },
    )
    .await;
    match launched {
        Ok(()) => {}
        Err(cctui_server_core::launch::LaunchError::Delivery(err)) => {
            if let Err(e) =
                crate::routes::attachments::delete_attachments(&state.pool, &recorded_ids).await
            {
                tracing::warn!(session = %token_session_id, "undoing bootstrap attachments: {e}");
            }
            return Err(dispatch_error(&err));
        }
        Err(cctui_server_core::launch::LaunchError::Refused(detail)) => {
            tracing::warn!(machine = %req.machine_id, %command_id, %adapter_id, %detail, "spawn refused by the daemon");
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ApiError { error: format!("the daemon refused the spawn: {detail}") }),
            ));
        }
    }

    tracing::info!(machine = %req.machine_id, %command_id, %adapter_id, "spawn dispatched");
    Ok((
        StatusCode::ACCEPTED,
        Json(SpawnResponse {
            command_id,
            status: "dispatched".into(),
            account: account_choice,
            session_id: pre_session_id,
        }),
    ))
}

/// The 503 a spawn answers when its command never reached the daemon.
fn dispatch_error(err: &crate::bus::BusError) -> (StatusCode, Json<ApiError>) {
    let error = match err {
        crate::bus::BusError::NoDaemon(_) => {
            "daemon for that machine is offline — start `cctui-daemon` first"
        }
        _ => "daemon disconnected mid-dispatch",
    };
    (StatusCode::SERVICE_UNAVAILABLE, Json(ApiError { error: error.into() }))
}

/// The instance default capability, granting only the harnesses `machine_uuid` runs.
///
/// Falls back to the default-enabled set when the machine's rows cannot be read.
use cctui_server_core::machine_policy::persist_spawn_capability;

/// Resolve `machine_id` (a UUID) to `(machine_uuid, owner)`.
pub use cctui_server_core::machine_policy::{authorize_launch, resolve_owned_machine};

/// [`authorize_launch`] on the machine `session_id` runs on. A session with no
/// machine still needs the `dispatch` scope.
pub async fn authorize_session_launch(
    state: &AppState,
    ctx: &AuthContext,
    session_id: &str,
) -> Result<(), AppError> {
    let machine: Option<Option<Uuid>> =
        sqlx::query_scalar("SELECT machine_uuid FROM sessions WHERE id = $1")
            .bind(session_id)
            .fetch_optional(&state.pool)
            .await?;
    match machine.flatten() {
        Some(machine_uuid) => authorize_launch(state, ctx, machine_uuid).await,
        None => require_dispatch(ctx).map_err(|(code, Json(e))| AppError::new(code, e.error)),
    }
}

/// A fork inherits its parent's account. Refused when the parent runs on
/// another user's account (the machine owner forking a spawn grantee's
/// session), so that account only ever serves the session it was bound to.
pub async fn refuse_foreign_account(
    state: &AppState,
    ctx: &AuthContext,
    session_id: &str,
) -> Result<(), AppError> {
    if ctx.is_admin() {
        return Ok(());
    }
    match crate::routes::gateway::acting_user(state, session_id, None).await {
        Some(runs_for) if runs_for != ctx.user_id => Err(AppError::new(
            StatusCode::FORBIDDEN,
            "this session runs on another user's account and cannot be forked",
        )),
        _ => Ok(()),
    }
}

/// Record `user_id` as the spawner of a session that has none. Revival paths
/// (resume) fill the gap but never rewrite who first started the session.
pub use cctui_server_core::machine_policy::{claim_spawner, fill_spawner, remember_spawner};

/// Persist a spawn payload as a `draft` session row. No env is stored
/// (re-entered at launch), no daemon dispatch happens, and the row is excluded
/// from liveness/reaping via its sticky `draft` status. Attached files are kept
/// in the blob store under the draft id so the launch can stage them. Returns
/// the new draft session id in `command_id` with `status = "draft"`.
async fn save_draft(
    state: &AppState,
    ctx: &AuthContext,
    req: &SpawnRequest,
    uploads: &[crate::uploads::RawUpload],
) -> Result<(StatusCode, Json<SpawnResponse>), AppError> {
    let (machine_uuid, _) = resolve_owned_machine(state, ctx, &req.machine_id)
        .await
        .map_err(|(code, Json(e))| AppError::new(code, e.error))?;
    let adapter_id = req.adapter_id.clone().unwrap_or_else(|| "claude-code".to_owned());

    // Store the spawn config (NOT env — secrets never persisted) under
    // `metadata.draft` so Launch/Edit can reconstruct the SpawnRequest.
    let mut payload = req.clone();
    payload.env.clear();
    payload.save_draft = false;
    let draft_json = serde_json::to_value(&payload).map_err(|e| {
        tracing::error!("serializing draft payload: {e}");
        AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "could not encode draft")
    })?;
    let metadata = serde_json::json!({ "draft": draft_json });

    let draft_id = Uuid::new_v4();
    let name = req.name.as_deref().filter(|n| !n.trim().is_empty());
    let model = req.model.as_deref().filter(|m| !m.trim().is_empty());
    let effort = req.effort.as_deref().filter(|e| !e.trim().is_empty());
    sqlx::query(
        r"INSERT INTO sessions
            (id, machine_id, machine_uuid, working_dir, status, registered_at, last_heartbeat,
             metadata, adapter_id, session_name, model, effort)
          VALUES ($1, $2, $3, $4, 'draft', now(), now(), $5, $6, $7, $8, $9)",
    )
    .bind(draft_id)
    .bind(&req.machine_id)
    .bind(machine_uuid)
    .bind(&req.working_dir)
    .bind(&metadata)
    .bind(&adapter_id)
    .bind(name)
    .bind(model)
    .bind(effort)
    .execute(&state.pool)
    .await?;

    let draft_key = draft_id.to_string();
    if !uploads.is_empty() {
        crate::routes::attachments::record_uploads(&state.pool, &draft_key, uploads, &[])
            .await
            .map_err(|e| {
                tracing::error!(draft = %draft_key, "recording draft attachments: {e}");
                AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "could not store the attachments")
            })?;
    }
    crate::spawn_labels::sync_draft(&state.pool, &draft_key, &req.label_ids, ctx.owner_filter())
        .await;
    tracing::info!(machine = %req.machine_id, draft = %draft_id, files = uploads.len(), "draft session saved");
    Ok((
        StatusCode::CREATED,
        Json(SpawnResponse {
            command_id: draft_id,
            status: SessionRowStatus::Draft.as_str().into(),
            account: None,
            session_id: None,
        }),
    ))
}

/// `POST /api/v1/sessions/{id}/launch`. Promote a draft to a live
/// spawn: read the stored payload, merge the freshly-entered env, dispatch the
/// real spawn (minting account gateway env), then delete the draft row. The
/// live session appears via the daemon's normal registration.
pub async fn launch_draft(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(session_id): Path<String>,
    Json(launch): Json<LaunchRequest>,
) -> Result<(StatusCode, Json<SpawnResponse>), AppError> {
    launch_stored_draft(&state, &ctx, &session_id, launch.env).await
}

/// Shared with the scheduled-launch sweep: a due draft must go out through the
/// same path as a hand-launched one.
pub async fn launch_stored_draft(
    state: &AppState,
    ctx: &AuthContext,
    session_id: &str,
    env: std::collections::BTreeMap<String, String>,
) -> Result<(StatusCode, Json<SpawnResponse>), AppError> {
    let row: Option<(String, serde_json::Value)> =
        sqlx::query_as("SELECT status, metadata FROM sessions WHERE id = $1")
            .bind(session_id)
            .fetch_optional(&state.pool)
            .await?;
    let Some((status, metadata)) = row else {
        return Err(AppError::new(StatusCode::NOT_FOUND, "draft not found"));
    };
    if SessionRowStatus::parse(&status) != Some(SessionRowStatus::Draft) {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "session is not a draft"));
    }
    let mut req: SpawnRequest = metadata
        .get("draft")
        .cloned()
        .ok_or_else(|| AppError::new(StatusCode::BAD_REQUEST, "draft row missing payload"))
        .and_then(|v| {
            serde_json::from_value(v).map_err(|e| {
                AppError::new(StatusCode::BAD_REQUEST, format!("corrupt draft payload: {e}"))
            })
        })?;
    // Env is entered fresh at launch; account gateway env is minted in dispatch.
    req.env = env;
    req.save_draft = false;
    // Labels put on the draft card after it was saved travel with the launch.
    for id in crate::spawn_labels::draft_label_ids(&state.pool, session_id).await {
        if !req.label_ids.contains(&id) {
            req.label_ids.push(id);
        }
    }

    let (uploads, raw_uploads) =
        load_draft_uploads(&state.pool, session_id, &req.attachment_names).await?;
    req.attachment_names.clear();

    let outcome = dispatch_spawn(state, ctx, req, uploads, raw_uploads)
        .await
        .map_err(|(code, Json(e))| AppError::new(code, e.error))?;

    // Drop the draft only after a successful dispatch; the live session is born
    // from the daemon's registration with its own id. The `sessions` delete
    // trigger drops the draft's attachment rows with it.
    if let Err(e) = sqlx::query("DELETE FROM sessions WHERE id = $1 AND status = 'draft'")
        .bind(session_id)
        .execute(&state.pool)
        .await
    {
        tracing::warn!(%session_id, "draft launched but row delete failed: {e}");
    }
    tracing::info!(draft = %session_id, "draft launched");
    crate::events::record(
        state,
        crate::events::Event::new(
            crate::events::kind::SESSION_LAUNCHED,
            crate::events::Actor::User(ctx.user_id),
        )
        .user(ctx.user_id)
        .detail(serde_json::json!({
            "draft_id": session_id,
            "session_id": outcome.1.session_id,
            "command_id": outcome.1.command_id,
        })),
    );
    Ok(outcome)
}

/// The draft's stored files, re-read from the blob store in the shape
/// [`dispatch_spawn`] takes, so the launched session stages and records them
/// exactly like a direct spawn. `expected` are the names the draft payload
/// references: any of them absent from the store fails the launch rather than
/// spawning a prompt that points at files the worker never gets.
pub async fn load_draft_uploads(
    pool: &sqlx::PgPool,
    draft_id: &str,
    expected: &[String],
) -> Result<(Vec<cctui_proto::adapter::BootstrapFile>, Vec<crate::uploads::RawUpload>), AppError> {
    use base64::Engine;
    let rows = crate::routes::attachments::list_session_attachments(pool, draft_id).await?;
    let missing: Vec<&str> = expected
        .iter()
        .filter(|n| !rows.iter().any(|r| r.name == **n))
        .map(String::as_str)
        .collect();
    if !missing.is_empty() {
        return Err(AppError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            format!(
                "draft attachments missing from storage: {}; re-attach them and save the draft",
                missing.join(", ")
            ),
        ));
    }
    let mut uploads = Vec::with_capacity(rows.len());
    let mut raw = Vec::with_capacity(rows.len());
    for row in rows {
        let Some((_, bytes)) =
            crate::routes::blobs::fetch_session_blob(pool, draft_id, &row.hash).await?
        else {
            return Err(AppError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                format!(
                    "draft attachment {:?} has no stored content; re-attach it and save the draft",
                    row.name
                ),
            ));
        };
        uploads.push(cctui_proto::adapter::BootstrapFile {
            name: row.name.clone(),
            content_b64: base64::engine::general_purpose::STANDARD.encode(&bytes),
        });
        raw.push(crate::uploads::RawUpload {
            name: row.name,
            bytes: bytes.into(),
            content_type: row.content_type,
        });
    }
    Ok((uploads, raw))
}

/// `PUT /api/v1/sessions/{id}/draft-attachments` — `multipart/form-data`.
///
/// Replace a draft's stored files with the parts of this request (same shape
/// and caps as `/sessions/spawn`; no file parts clears them). The webui sends
/// it only when the form's file set changed since the last save, so
/// `PUT /sessions/{id}/draft` stays a cheap JSON body.
pub async fn put_draft_attachments(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    multipart: Multipart,
) -> Result<Json<Vec<crate::routes::attachments::SessionAttachment>>, AppError> {
    let parsed = parse_upload_multipart(multipart, cached_upload_caps(&state))
        .await
        .map_err(|(code, Json(e))| AppError::new(code, e.error))?;
    let status: Option<String> = sqlx::query_scalar("SELECT status FROM sessions WHERE id = $1")
        .bind(&session_id)
        .fetch_optional(&state.pool)
        .await?;
    let Some(status) = status else {
        return Err(AppError::new(StatusCode::NOT_FOUND, "draft not found"));
    };
    if SessionRowStatus::parse(&status) != Some(SessionRowStatus::Draft) {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "session is not a draft"));
    }
    let rows =
        replace_draft_attachments(&state.pool, &session_id, &parsed.raw).await.map_err(|e| {
            tracing::error!(draft = %session_id, "replacing draft attachments: {e}");
            AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "could not store the attachments")
        })?;
    tracing::info!(draft = %session_id, files = rows.len(), "draft attachments replaced");
    Ok(Json(rows))
}

/// Swap a draft's attachment rows for `uploads` in one transaction. Blobs are
/// content-addressed and shared, so only the rows go.
pub async fn replace_draft_attachments(
    pool: &sqlx::PgPool,
    draft_id: &str,
    uploads: &[crate::uploads::RawUpload],
) -> Result<Vec<crate::routes::attachments::SessionAttachment>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM session_attachments WHERE session_id = $1")
        .bind(draft_id)
        .execute(&mut *tx)
        .await?;
    let rows =
        crate::routes::attachments::record_uploads_in(&mut tx, draft_id, uploads, &[]).await?;
    tx.commit().await?;
    Ok(rows)
}

/// `POST /api/v1/sessions/{id}/schedule-launch`. Queue a draft to launch at
/// `launch_at`; replaces any schedule already on it.
pub async fn schedule_draft_launch(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(session_id): Path<String>,
    Json(body): Json<ScheduleLaunchRequest>,
) -> Result<StatusCode, AppError> {
    require_dispatch(&ctx).map_err(|(code, Json(e))| AppError::new(code, e.error))?;
    let at = crate::scheduled_messages::parse_deliver_at(&body.launch_at, chrono::Utc::now())
        .map_err(|e| AppError::new(StatusCode::BAD_REQUEST, e.to_string()))?;
    let status: Option<String> = sqlx::query_scalar("SELECT status FROM sessions WHERE id = $1")
        .bind(&session_id)
        .fetch_optional(&state.pool)
        .await?;
    let Some(status) = status else {
        return Err(AppError::new(StatusCode::NOT_FOUND, "draft not found"));
    };
    if SessionRowStatus::parse(&status) != Some(SessionRowStatus::Draft) {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "session is not a draft"));
    }
    crate::scheduled_spawns::schedule(&state.pool, &state.bus, &session_id, Some(ctx.user_id), at)
        .await?;
    tracing::info!(draft = %session_id, launch_at = %at, "draft launch scheduled");
    Ok(StatusCode::ACCEPTED)
}

/// `POST /api/v1/sessions/{id}/cancel-launch`. Drop a draft's schedule and
/// leave the draft itself alone.
pub async fn cancel_draft_launch(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<StatusCode, AppError> {
    if !crate::scheduled_spawns::cancel(&state.pool, &state.bus, &session_id).await? {
        return Err(AppError::new(StatusCode::NOT_FOUND, "draft is not scheduled"));
    }
    tracing::info!(draft = %session_id, "draft launch schedule cancelled");
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/sessions/{id}/discard`. Delete a draft session row.
/// Only acts on `draft` rows so it can never delete a real session.
pub async fn discard_draft(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<StatusCode, AppError> {
    let res = sqlx::query("DELETE FROM sessions WHERE id = $1 AND status = 'draft'")
        .bind(&session_id)
        .execute(&state.pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::new(StatusCode::NOT_FOUND, "draft not found"));
    }
    tracing::info!(draft = %session_id, "draft discarded");
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/sessions/{id}/files` — `multipart/form-data`.
///
/// Mid-chat file attachments. Same multipart shape + caps as `/sessions/spawn`
/// (one shared helper, [`crate::uploads::parse_upload_multipart`]); files are
/// forwarded to the owning daemon over the existing WS as a `StageFiles` op and
/// staged into the same per-session dir used at spawn time. Returns the staged
/// absolute paths so the client can reference them under the reply prompt.
pub async fn stage_session_files(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    multipart: Multipart,
) -> Result<Json<cctui_proto::api::StageFilesResponse>, AppError> {
    let parsed = parse_upload_multipart(multipart, cached_upload_caps(&state))
        .await
        .map_err(|(code, Json(e))| AppError::new(code, e.error))?;
    if parsed.files.is_empty() {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "no files in upload"));
    }
    let count = parsed.files.len();
    // Same ordering as the spawn path: store the blobs first so a blob-store
    // failure fails the request before any file reaches the machine.
    let recorded =
        crate::routes::attachments::record_uploads(&state.pool, &session_id, &parsed.raw, &[])
            .await
            .map_err(|e| {
                tracing::error!(%session_id, "recording attachments: {e}");
                AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "could not store the attachments")
            })?;
    let recorded_ids: Vec<Uuid> = recorded.iter().map(|a| a.id).collect();

    let staged = crate::bus::stage_files(&state, &session_id, parsed.files).await;
    if staged.is_err()
        && let Err(e) =
            crate::routes::attachments::delete_attachments(&state.pool, &recorded_ids).await
    {
        tracing::warn!(%session_id, "undoing mid-chat attachments: {e}");
    }
    match staged {
        Ok(paths) => {
            tracing::info!(%session_id, count, "staged mid-chat files");
            let names: Vec<String> =
                paths.iter().map(|p| p.rsplit('/').next().unwrap_or(p).to_owned()).collect();
            if let Err(e) =
                crate::routes::attachments::set_attachment_names(&state.pool, &recorded, &names)
                    .await
            {
                tracing::error!(%session_id, "adopting staged attachment names: {e}");
            }
            Ok(Json(cctui_proto::api::StageFilesResponse { paths }))
        }
        Err(crate::bus::BusError::NotFound) => {
            Err(AppError::new(StatusCode::NOT_FOUND, "session not found"))
        }
        Err(err @ (crate::bus::BusError::NoDaemon(_) | crate::bus::BusError::Closed)) => {
            Err(AppError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                format!("{err} — the session's machine is offline; try again"),
            ))
        }
        Err(crate::bus::BusError::Timeout) => Err(AppError::new(
            StatusCode::GATEWAY_TIMEOUT,
            "timed out staging files on the session's machine",
        )),
        Err(err @ crate::bus::BusError::Staging(_)) => {
            Err(AppError::new(StatusCode::BAD_GATEWAY, err.to_string()))
        }
        Err(err) => {
            tracing::error!(%session_id, %err, "stage_files dispatch error");
            Err(AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "could not stage files"))
        }
    }
}

/// Poll endpoint superseded by WS push; answers an empty list so polling
/// clients don't get a 404.
/// Draining the queue consumes it, so only the machine's owner may poll it.
pub async fn get_machine_commands(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(machine_id): Path<String>,
) -> Result<Json<Vec<MachineCommand>>, AppError> {
    let owned = match Uuid::parse_str(&machine_id) {
        Ok(id) => {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (SELECT 1 FROM machines WHERE id = $1 AND user_id = $2)",
            )
            .bind(id)
            .bind(ctx.user_id)
            .fetch_one(&state.pool)
            .await?
        }
        Err(_) => false,
    };
    if !owned {
        return Err(AppError::new(StatusCode::NOT_FOUND, "machine not found"));
    }
    let commands = {
        let mut registry = state.registry.write().await;
        registry.take_machine_commands(&machine_id)
    };
    Ok(Json(commands))
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use base64::Engine;

    /// An enabled adapter outside the harness table is not a family the server
    /// can launch, so the default capability must not advertise it to
    /// `CctuiAgent`.
    #[test]
    fn the_default_capability_advertises_only_spawnable_adapters() {
        let ids = ["claude-code", "gemini", "codex", "opencode", "qwen", "goose", "custom-acp"]
            .map(str::to_owned)
            .to_vec();
        assert_eq!(
            super::spawnable_adapters(ids),
            ["claude-code", "gemini", "codex", "opencode", "qwen", "goose"]
        );
        assert!(super::spawnable_adapters(Vec::new()).is_empty());
    }

    #[tokio::test]
    async fn only_the_owner_drains_a_machine_queue() {
        use axum::extract::{Path, State};
        use axum::{Extension, Json};

        let Some(url) =
            crate::routes::gateway::test_db_url("only_the_owner_drains_a_machine_queue")
        else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
        let (owner, other, machine) =
            (uuid::Uuid::new_v4(), uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
        for uid in [owner, other] {
            sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
                .bind(uid)
                .bind(format!("mq-{uid}"))
                .bind(format!("hmq-{uid}"))
                .execute(&pool)
                .await
                .unwrap();
        }
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, 'm', $3)")
            .bind(machine)
            .bind(owner)
            .bind(format!("mkmq-{machine}"))
            .execute(&pool)
            .await
            .unwrap();
        let state = crate::state::AppState::for_test(pool);
        state.registry.write().await.queue_machine_command(
            &machine.to_string(),
            "noop",
            serde_json::Value::Null,
        );
        let caller = |user_id| crate::auth::AuthContext {
            user_id,
            key_id: uuid::Uuid::new_v4(),
            machine_id: None,
            scopes: std::iter::once(crate::auth::Scope::Read).collect(),
        };

        let denied = super::get_machine_commands(
            State(state.clone()),
            Extension(caller(other)),
            Path(machine.to_string()),
        )
        .await;
        assert!(denied.is_err(), "another user must not drain the queue");

        let Json(drained) = super::get_machine_commands(
            State(state),
            Extension(caller(owner)),
            Path(machine.to_string()),
        )
        .await
        .unwrap();
        assert_eq!(drained.len(), 1, "the refused poll left the command queued");
    }

    fn upload(name: &str, bytes: &[u8]) -> crate::uploads::RawUpload {
        crate::uploads::RawUpload {
            name: name.to_owned(),
            bytes: axum::body::Bytes::copy_from_slice(bytes),
            content_type: None,
        }
    }

    async fn seed_draft(pool: &sqlx::PgPool) -> String {
        let uid = uuid::Uuid::new_v4();
        let machine = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, 'draft-att', $2)")
            .bind(uid)
            .bind(format!("kh-{uid}"))
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, $3, $4)")
            .bind(machine)
            .bind(uid)
            .bind(machine.to_string())
            .bind(format!("kh-{machine}"))
            .execute(pool)
            .await
            .unwrap();
        let sid = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, working_dir, user_id, machine_uuid, adapter_id, status) \
             VALUES ($1, $2, '/w', $3, $4, 'claude-code', 'draft')",
        )
        .bind(&sid)
        .bind(machine.to_string())
        .bind(uid)
        .bind(machine)
        .execute(pool)
        .await
        .unwrap();
        sid
    }

    #[tokio::test]
    async fn a_launch_reads_the_drafts_files_back_as_spawn_uploads() {
        let Some(url) = crate::routes::gateway::test_db_url("draft_attachments_launch") else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
        let draft = seed_draft(&pool).await;
        let note = format!("draft note {draft}");
        crate::routes::attachments::record_uploads(
            &pool,
            &draft,
            &[
                upload("note.md", note.as_bytes()),
                upload("shot.png", &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1]),
            ],
            &[],
        )
        .await
        .unwrap();

        let names = ["note.md".to_owned(), "shot.png".to_owned()];
        let (uploads, raw) = super::load_draft_uploads(&pool, &draft, &names).await.unwrap();
        assert_eq!(uploads.len(), 2);
        assert_eq!(raw.len(), 2);
        assert_eq!(uploads[0].name, "note.md");
        assert_eq!(
            base64::engine::general_purpose::STANDARD.decode(&uploads[0].content_b64).unwrap(),
            note.as_bytes(),
            "the daemon decodes the same bytes the form attached"
        );
        assert_eq!(raw[1].name, "shot.png");
        assert_eq!(raw[1].content_type.as_deref(), Some("image/png"));

        let Err(crate::error::AppError::Status(code, msg)) =
            super::load_draft_uploads(&pool, &draft, &["gone.txt".to_owned()]).await
        else {
            panic!("a referenced file the store lacks must fail the launch");
        };
        assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(msg.contains("gone.txt"), "{msg}");

        let rows = super::replace_draft_attachments(&pool, &draft, &[upload("only.txt", b"x")])
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        let listed =
            crate::routes::attachments::list_session_attachments(&pool, &draft).await.unwrap();
        assert_eq!(listed.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["only.txt"]);
        let (uploads, _) = super::load_draft_uploads(&pool, &draft, &[]).await.unwrap();
        assert_eq!(uploads.len(), 1, "a draft with no listed names still ships what it stored");

        super::replace_draft_attachments(&pool, &draft, &[]).await.unwrap();
        assert!(
            crate::routes::attachments::list_session_attachments(&pool, &draft)
                .await
                .unwrap()
                .is_empty()
        );
        let (uploads, raw) = super::load_draft_uploads(&pool, &draft, &[]).await.unwrap();
        assert!(uploads.is_empty() && raw.is_empty());

        sqlx::query("DELETE FROM sessions WHERE id = $1")
            .bind(&draft)
            .execute(&pool)
            .await
            .unwrap();
    }

    #[test]
    fn spawn_refuses_an_adapter_with_no_family() {
        assert_eq!(super::spawn_adapter(None).unwrap(), "claude-code");
        assert_eq!(super::spawn_adapter(Some(" ")).unwrap(), "claude-code");
        for adapter in ["claude-code", "codex", "opencode", "gemini", "qwen", "goose"] {
            assert_eq!(super::spawn_adapter(Some(adapter)).unwrap(), adapter);
        }
        assert!(super::spawn_adapter(Some("kilo")).is_err());
    }

    #[test]
    fn every_spawn_hands_the_daemon_the_key_its_token_is_bound_to() {
        let command_id = uuid::Uuid::new_v4();
        for adapter in ["codex", "opencode"] {
            let (pre, key) = super::spawn_ids(adapter, command_id);
            assert_eq!(pre, None, "{adapter} registers under its own id, none is promised");
            assert_eq!(key, command_id, "{adapter}'s token is keyed on the dispatch");
        }
        let (pre, key) = super::spawn_ids("claude-code", command_id);
        assert_eq!(pre, Some(key), "claude registers under the pre-minted id");
        assert_ne!(key, command_id);
    }

    use super::{AccountDecision, decide_account, resolve_default_account};
    use uuid::Uuid;

    #[test]
    fn named_account_always_wins() {
        assert_eq!(
            decide_account(Some(" acme "), false, false, None),
            AccountDecision::Named("acme".to_owned())
        );
        assert_eq!(
            decide_account(Some("acme"), true, false, None),
            AccountDecision::Named("acme".to_owned())
        );
        // A stale `auto_account` can no more override an explicit pick than a
        // stale `no_account` can.
        assert_eq!(
            decide_account(Some("acme"), false, true, None),
            AccountDecision::Named("acme".to_owned())
        );
    }

    #[test]
    fn no_account_bypasses_default_resolution() {
        assert_eq!(decide_account(None, true, false, None), AccountDecision::Unbound);
        assert_eq!(decide_account(Some("   "), true, false, None), AccountDecision::Unbound);
        // An explicit unbound spawn outranks auto-selection: asking for no
        // account can never be answered with one.
        assert_eq!(decide_account(None, true, true, None), AccountDecision::Unbound);
    }

    #[test]
    fn unset_account_resolves_default() {
        assert_eq!(decide_account(None, false, false, None), AccountDecision::ResolveDefault);
        assert_eq!(decide_account(Some(""), false, false, None), AccountDecision::ResolveDefault);
    }

    #[test]
    fn auto_account_delegates_the_choice() {
        assert_eq!(decide_account(None, false, true, None), AccountDecision::Auto);
        assert_eq!(decide_account(Some("  "), false, true, None), AccountDecision::Auto);
    }

    #[test]
    fn a_pool_bounds_the_choice_and_beats_auto() {
        assert_eq!(
            decide_account(None, false, false, Some(" perso ")),
            AccountDecision::Pool("perso".to_owned())
        );
        // `auto_account` widens to every reachable account; a named pool is
        // the narrower instruction, so it must win rather than be widened.
        assert_eq!(
            decide_account(None, false, true, Some("perso")),
            AccountDecision::Pool("perso".to_owned())
        );
        // The two refusals still outrank it: an explicit account, and an
        // explicit unbound spawn.
        assert_eq!(
            decide_account(Some("acme"), false, false, Some("perso")),
            AccountDecision::Named("acme".to_owned())
        );
        assert_eq!(decide_account(None, true, false, Some("perso")), AccountDecision::Unbound);
        // A blank pool name is not a pool.
        assert_eq!(decide_account(None, false, true, Some("  ")), AccountDecision::Auto);
        assert_eq!(decide_account(None, false, false, Some("")), AccountDecision::ResolveDefault);
    }

    /// The reauth gate lives in the election only. An account named outright
    /// never reaches it, whatever else the request asks for, so naming an
    /// account that must be reconnected still binds it, exactly as before.
    #[test]
    fn a_named_account_is_never_put_to_an_election() {
        for (auto, pool) in [(false, None), (true, None), (true, Some("perso"))] {
            assert_eq!(
                decide_account(Some("refused"), false, auto, pool),
                AccountDecision::Named("refused".to_owned())
            );
        }
        assert_eq!(
            crate::account_resolve::choose_target(
                "refused",
                true,
                Some(crate::store::account_pools::AccountPool {
                    id: Uuid::nil(),
                    user_id: Uuid::nil(),
                    name: "refused".to_owned(),
                    strategy: crate::store::account_pools::STRATEGY_HEADROOM.to_owned(),
                    failover: false,
                    created_at: chrono::Utc::now(),
                })
            ),
            crate::account_resolve::Target::Account("refused".to_owned())
        );
    }

    #[test]
    fn default_account_zero_one_many() {
        let uid = Uuid::nil();
        assert_eq!(resolve_default_account(&[], uid, "codex").unwrap(), None);
        assert_eq!(
            resolve_default_account(&["solo".to_owned()], uid, "codex").unwrap(),
            Some("solo".to_owned())
        );
        let err =
            resolve_default_account(&["a".to_owned(), "b".to_owned()], uid, "codex").unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
        assert!(err.1.0.error.contains("a, b"));
    }

    /// An account whose owner cleared `pool_eligible` is out of every election
    /// (`auto_account`, its owned-only variant for `CctuiAgent` children, and
    /// pools, which gateway failover elects through), owned or shared, while
    /// the eligible ones next to it stay candidates.
    #[tokio::test]
    async fn an_ineligible_account_is_never_elected() {
        use crate::routes::gateway::Family;

        let Some(url) =
            crate::routes::gateway::test_db_url("an_ineligible_account_is_never_elected")
        else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");

        let me = elig_user(&pool).await;
        let lender = elig_user(&pool).await;
        let spark = elig_account(&pool, me, "Spark local (OpenCode)", "fireworks", false).await;
        let cloud = elig_account(&pool, me, "Fireworks cloud", "fireworks", true).await;
        let lent_spark = elig_account(&pool, lender, "Lent spark", "fireworks", false).await;
        let lent_cloud = elig_account(&pool, lender, "Lent cloud", "fireworks", true).await;
        for lent in [lent_spark, lent_cloud] {
            sqlx::query(
                "INSERT INTO resource_shares (resource_type, resource_id, grantee_id) \
                 VALUES ('account', $1, $2)",
            )
            .bind(lent)
            .bind(me)
            .execute(&pool)
            .await
            .unwrap();
        }

        let names = |rows: Vec<super::AutoAccountRow>| {
            let mut n: Vec<String> = rows.into_iter().map(|r| r.1).collect();
            n.sort();
            n
        };
        let auto =
            super::auto_account_candidates(&pool, me, Family::Fireworks, false).await.unwrap();
        assert_eq!(names(auto), ["Fireworks cloud", "Lent cloud"]);
        let owned =
            super::auto_account_candidates(&pool, me, Family::Fireworks, true).await.unwrap();
        assert_eq!(names(owned), ["Fireworks cloud"]);

        // Enrolled in the owner's own pool, the ineligible account is kept as a
        // member but never handed to an election.
        let pool_id: uuid::Uuid = sqlx::query_scalar(
            "INSERT INTO account_pools (user_id, name) VALUES ($1, 'Mine') RETURNING id",
        )
        .bind(me)
        .fetch_one(&pool)
        .await
        .unwrap();
        for (pos, acct) in [spark, cloud, lent_spark, lent_cloud].into_iter().enumerate() {
            sqlx::query(
                "INSERT INTO account_pool_members (pool_id, account_id, position) \
                 VALUES ($1, $2, $3)",
            )
            .bind(pool_id)
            .bind(acct)
            .bind(i32::try_from(pos).unwrap())
            .execute(&pool)
            .await
            .unwrap();
        }
        let usable = crate::store::account_pools::usable_members(&pool, pool_id, me, "fireworks")
            .await
            .unwrap();
        let usable: Vec<&str> = usable.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(usable, ["Fireworks cloud", "Lent cloud"]);
        let stored = crate::store::account_pools::members(&pool, pool_id).await.unwrap();
        assert_eq!(stored.len(), 4, "membership rows are kept, only elections skip them");

        for uid in [me, lender] {
            sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
        }
    }

    async fn elig_user(pool: &sqlx::PgPool) -> uuid::Uuid {
        let uid = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("elig-{uid}"))
            .bind(format!("helig-{uid}"))
            .execute(pool)
            .await
            .unwrap();
        uid
    }
    async fn elig_account(
        pool: &sqlx::PgPool,
        owner: uuid::Uuid,
        name: &str,
        provider: &str,
        eligible: bool,
    ) -> uuid::Uuid {
        let id: uuid::Uuid = sqlx::query_scalar(
            "INSERT INTO accounts (user_id, name, pool_eligible) VALUES ($1, $2, $3) \
             RETURNING id",
        )
        .bind(owner)
        .bind(name)
        .bind(eligible)
        .fetch_one(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO account_providers (user_id, account_id, provider) \
             VALUES ($1, $2, $3)",
        )
        .bind(owner)
        .bind(id)
        .bind(provider)
        .execute(pool)
        .await
        .unwrap();
        id
    }
}

#[cfg(test)]
mod spawn_share_tests {
    use axum::http::StatusCode;
    use uuid::Uuid;

    use crate::auth::{AuthContext, Scope};

    pub(super) fn caller(user_id: Uuid, scopes: &[Scope]) -> AuthContext {
        AuthContext {
            user_id,
            key_id: Uuid::new_v4(),
            machine_id: None,
            scopes: scopes.iter().copied().collect(),
        }
    }

    pub(super) async fn seed_user(pool: &sqlx::PgPool) -> Uuid {
        let uid = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("share-spawn-{uid}"))
            .bind(format!("hss-{uid}"))
            .execute(pool)
            .await
            .unwrap();
        uid
    }

    pub(super) async fn seed_machine(pool: &sqlx::PgPool, owner: Uuid) -> Uuid {
        let machine = Uuid::new_v4();
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, 'm', $3)")
            .bind(machine)
            .bind(owner)
            .bind(format!("mkss-{machine}"))
            .execute(pool)
            .await
            .unwrap();
        machine
    }

    pub(super) async fn share(pool: &sqlx::PgPool, machine: Uuid, grantee: Uuid, action: &str) {
        sqlx::query(
            "INSERT INTO resource_shares (resource_type, resource_id, grantee_id, action) \
             VALUES ('machine', $1, $2, $3)",
        )
        .bind(machine)
        .bind(grantee)
        .bind(action)
        .execute(pool)
        .await
        .unwrap();
    }

    pub(super) const SPAWNER: &[Scope] = &[Scope::Read, Scope::Enroll, Scope::Dispatch];
    pub(super) const ADMIN: &[Scope] = &[Scope::Read, Scope::Enroll, Scope::Dispatch, Scope::Admin];

    #[tokio::test]
    async fn a_spawn_needs_the_dispatch_scope() {
        let pool = sqlx::PgPool::connect_lazy("postgres://invalid").unwrap();
        let state = crate::state::AppState::for_test(pool);
        let req: cctui_proto::api::SpawnRequest = serde_json::from_value(serde_json::json!({
            "machine_id": Uuid::new_v4().to_string(),
            "working_dir": "/w",
            "permission_mode": "yolo",
        }))
        .unwrap();
        let read_only = caller(Uuid::new_v4(), &[Scope::Read]);
        let refused = super::dispatch_spawn(&state, &read_only, req, Vec::new(), Vec::new()).await;
        assert_eq!(refused.err().map(|(code, _)| code), Some(StatusCode::FORBIDDEN));
    }

    #[tokio::test]
    async fn only_a_spawn_share_lets_a_non_owner_spawn() {
        let Some(url) =
            crate::routes::gateway::test_db_url("only_a_spawn_share_lets_a_non_owner_spawn")
        else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
        let owner = seed_user(&pool).await;
        let grantee = seed_user(&pool).await;
        let reader = seed_user(&pool).await;
        let stranger = seed_user(&pool).await;
        let machine = seed_machine(&pool, owner).await;
        share(&pool, machine, grantee, "spawn").await;
        share(&pool, machine, reader, "use").await;
        let state = crate::state::AppState::for_test(pool.clone());
        let id = machine.to_string();
        let resolve = |user, scopes| {
            let state = state.clone();
            let id = id.clone();
            async move {
                super::resolve_spawn_machine(&state, &caller(user, scopes), &id)
                    .await
                    .map_err(|(code, _)| code)
            }
        };

        assert_eq!(resolve(owner, SPAWNER).await, Ok((machine, owner)));
        assert_eq!(resolve(Uuid::nil(), ADMIN).await, Ok((machine, owner)));
        assert_eq!(
            resolve(grantee, SPAWNER).await,
            Ok((machine, owner)),
            "a spawn grantee spawns, and the machine owner keeps the session"
        );
        assert_eq!(resolve(reader, SPAWNER).await, Err(StatusCode::FORBIDDEN));
        assert_eq!(resolve(stranger, SPAWNER).await, Err(StatusCode::FORBIDDEN));
        assert_eq!(
            super::resolve_owned_machine(&state, &caller(grantee, SPAWNER), &id)
                .await
                .map_err(|(code, _)| code),
            Err(StatusCode::FORBIDDEN),
            "drafts and other owner-only paths do not honour a spawn share"
        );

        sqlx::query(
            "UPDATE resource_shares SET revoked_at = now() \
             WHERE resource_type = 'machine' AND resource_id = $1 AND grantee_id = $2",
        )
        .bind(machine)
        .bind(grantee)
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(resolve(grantee, SPAWNER).await, Err(StatusCode::FORBIDDEN));
    }

    #[tokio::test]
    async fn the_spawner_is_recorded_and_the_owner_keeps_the_session() {
        let Some(url) = crate::routes::gateway::test_db_url(
            "the_spawner_is_recorded_and_the_owner_keeps_the_session",
        ) else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
        let owner = seed_user(&pool).await;
        let grantee = seed_user(&pool).await;
        let machine = seed_machine(&pool, owner).await;
        share(&pool, machine, grantee, "spawn").await;
        let spawn_key = Uuid::new_v4().to_string();
        let sid = Uuid::new_v4().to_string();

        super::remember_spawner(&pool, &spawn_key, grantee, None).await;
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status) \
             VALUES ($1, $2, $3, $4, '/w', 'new')",
        )
        .bind(&sid)
        .bind(machine.to_string())
        .bind(machine)
        .bind(owner)
        .execute(&pool)
        .await
        .unwrap();
        super::claim_spawner(&pool, &sid, Some(&spawn_key)).await;
        super::claim_spawner(&pool, &sid, None).await;

        let spawned_by: Option<Uuid> =
            sqlx::query_scalar("SELECT spawned_by FROM sessions WHERE id = $1")
                .bind(&sid)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(spawned_by, Some(grantee), "a second claim must not clear it");
        let left: i64 =
            sqlx::query_scalar("SELECT count(*) FROM session_spawner_intents WHERE spawn_key = $1")
                .bind(&spawn_key)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(left, 0, "the intent is consumed");

        assert_eq!(crate::authz::session_owner(&sid, &pool).await.unwrap(), Some(owner));
        assert_eq!(
            crate::authz::authorize_session_read(&caller(owner, SPAWNER), &sid, &pool).await,
            Ok(())
        );
        assert_eq!(
            crate::authz::authorize_session_read(&caller(grantee, SPAWNER), &sid, &pool).await,
            Err(StatusCode::FORBIDDEN),
            "spawning through a share does not make the grantee an owner"
        );
    }

    #[tokio::test]
    async fn a_spawn_name_is_pinned_when_the_session_registers() {
        let Some(url) = crate::routes::gateway::test_db_url(
            "a_spawn_name_is_pinned_when_the_session_registers",
        ) else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
        let owner = seed_user(&pool).await;
        let machine = seed_machine(&pool, owner).await;
        let register = |name: Option<&'static str>| {
            let pool = pool.clone();
            async move {
                let sid = Uuid::new_v4().to_string();
                super::remember_spawner(&pool, &sid, owner, name).await;
                sqlx::query(
                    "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, \
                     status) VALUES ($1, $2, $3, $4, '/w', 'new')",
                )
                .bind(&sid)
                .bind(machine.to_string())
                .bind(machine)
                .bind(owner)
                .execute(&pool)
                .await
                .unwrap();
                super::claim_spawner(&pool, &sid, None).await;
                let row: (Option<String>, Option<String>) = sqlx::query_as(
                    "SELECT session_name, metadata->>'name_source' FROM sessions WHERE id = $1",
                )
                .bind(&sid)
                .fetch_one(&pool)
                .await
                .unwrap();
                row
            }
        };

        assert_eq!(
            register(Some("  SUPER ORCHESTRATOR ")).await,
            (Some("SUPER ORCHESTRATOR".to_owned()), Some("user".to_owned()))
        );
        assert_eq!(register(None).await, (None, None), "no name, nothing pinned");
        assert_eq!(register(Some("   ")).await, (None, None), "a blank name is no name");
    }
}

#[cfg(test)]
mod launch_gate_tests {
    use axum::extract::{Path, State};
    use axum::http::StatusCode;
    use axum::{Extension, Json};
    use uuid::Uuid;

    use super::spawn_share_tests::{ADMIN, SPAWNER, caller, seed_machine, seed_user, share};
    use crate::auth::Scope;
    use crate::error::AppError;
    use crate::routes::gateway::Family;
    use crate::state::AppState;

    const READ_ONLY: &[Scope] = &[Scope::Read];

    async fn seed_session(pool: &sqlx::PgPool, owner: Uuid, machine: Uuid) -> String {
        let sid = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status, \
             adapter_id) VALUES ($1, $2, $3, $4, '/w', 'active', 'claude-code')",
        )
        .bind(&sid)
        .bind(machine.to_string())
        .bind(machine)
        .bind(owner)
        .execute(pool)
        .await
        .unwrap();
        sid
    }

    /// An anthropic account `user` owns; returns `(name, provider_id)`.
    async fn seed_account(pool: &sqlx::PgPool, user: Uuid) -> (String, Uuid) {
        let account = Uuid::new_v4();
        let name = format!("acct-{account}");
        sqlx::query("INSERT INTO accounts (id, user_id, name) VALUES ($1, $2, $3)")
            .bind(account)
            .bind(user)
            .bind(&name)
            .execute(pool)
            .await
            .unwrap();
        let provider = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO account_providers \
               (id, user_id, account_id, provider, auth_scheme, encrypted_access_token) \
             VALUES ($1, $2, $3, 'anthropic', 'api_key', 'x')",
        )
        .bind(provider)
        .bind(user)
        .bind(account)
        .execute(pool)
        .await
        .unwrap();
        (name, provider)
    }

    /// A session on `owner`'s machine that `grantee` spawned on their own
    /// account: minted for the grantee in the dispatch window, before the row.
    async fn seed_grantee_run(
        state: &AppState,
        owner: Uuid,
        grantee: Uuid,
        machine: Uuid,
        account: &str,
    ) -> String {
        let sid = Uuid::new_v4().to_string();
        crate::routes::gateway::mint_session_env(state, grantee, account, Family::Anthropic, &sid)
            .await
            .expect("mint for the grantee");
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status, \
             adapter_id, spawned_by) VALUES ($1, $2, $3, $4, '/w', 'active', 'claude-code', $5)",
        )
        .bind(&sid)
        .bind(machine.to_string())
        .bind(machine)
        .bind(owner)
        .bind(grantee)
        .execute(&state.pool)
        .await
        .unwrap();
        sid
    }

    fn code<T>(res: Result<T, AppError>) -> Result<(), StatusCode> {
        res.map(|_| ()).map_err(|e| e.into_parts().0)
    }

    fn daemon(
        state: &AppState,
        machine: Uuid,
    ) -> tokio::sync::mpsc::Receiver<cctui_proto::ws::DaemonFrameDown> {
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        state.bus.register_daemon(machine, Uuid::new_v4(), tx);
        rx
    }

    async fn fork(
        state: &AppState,
        ctx: crate::auth::AuthContext,
        sid: &str,
    ) -> Result<Option<String>, StatusCode> {
        crate::routes::sessions::fork_session(
            State(state.clone()),
            Extension(ctx),
            Path(sid.to_owned()),
            Json(cctui_proto::api::ForkRequest::default()),
        )
        .await
        .map(|(_, Json(res))| res.session_id)
        .map_err(|e| e.into_parts().0)
    }

    async fn resume(
        state: &AppState,
        ctx: crate::auth::AuthContext,
        sid: &str,
    ) -> Result<(), StatusCode> {
        code(
            crate::routes::sessions::resume_session(
                State(state.clone()),
                Extension(ctx),
                Path(sid.to_owned()),
            )
            .await,
        )
    }

    async fn spawned_by(pool: &sqlx::PgPool, sid: &str) -> Option<Uuid> {
        sqlx::query_scalar("SELECT spawned_by FROM sessions WHERE id = $1")
            .bind(sid)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// The child must already own a credential when the daemon receives Fork,
    /// including when reopening an archived parent whose tokens were revoked.
    #[tokio::test]
    async fn fork_binds_a_fresh_child_token_before_dispatch() {
        let Some(url) =
            crate::routes::gateway::test_db_url("fork_binds_a_fresh_child_token_before_dispatch")
        else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.unwrap();
        let owner = seed_user(&pool).await;
        let machine = seed_machine(&pool, owner).await;
        let (account, provider) = seed_account(&pool, owner).await;
        let state = AppState::for_test(pool.clone());
        let mut rx = daemon(&state, machine);
        for mode in ["live", "archived", "legacy", "unbound"] {
            let parent = seed_session(&pool, owner, machine).await;
            let parent_env = if mode == "unbound" {
                std::collections::BTreeMap::new()
            } else {
                crate::routes::gateway::mint_session_env(
                    &state,
                    owner,
                    &account,
                    Family::Anthropic,
                    &parent,
                )
                .await
                .unwrap()
            };
            if mode == "archived" {
                sqlx::query("UPDATE session_tokens SET revoked_at = now() WHERE session_id = $1")
                    .bind(&parent)
                    .execute(&pool)
                    .await
                    .unwrap();
                sqlx::query("UPDATE sessions SET status = 'archived' WHERE id = $1")
                    .bind(&parent)
                    .execute(&pool)
                    .await
                    .unwrap();
            } else if mode == "legacy" {
                sqlx::query("DELETE FROM session_tokens WHERE session_id = $1")
                    .bind(&parent)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            let child = fork(&state, caller(owner, SPAWNER), &parent).await.unwrap().unwrap();
            let cctui_proto::ws::DaemonFrameDown::Command { command, .. } =
                rx.recv().await.unwrap()
            else {
                panic!("expected a fork command")
            };
            let cctui_proto::adapter::AdapterCommand::Fork { spec, session_id, .. } = *command
            else {
                panic!("expected a fork")
            };
            assert_eq!(session_id.as_deref(), Some(child.as_str()));
            let bindings: Vec<(Uuid, Option<Uuid>)> = sqlx::query_as(
                "SELECT account_id, user_id FROM session_tokens \
                 WHERE session_id = $1 AND revoked_at IS NULL",
            )
            .bind(&child)
            .fetch_all(&pool)
            .await
            .unwrap();
            if mode == "unbound" {
                assert!(spec.env.is_empty());
                assert!(bindings.is_empty());
                continue;
            }
            assert_eq!(bindings, vec![(provider, Some(owner))]);
            assert_ne!(spec.env["ANTHROPIC_AUTH_TOKEN"], parent_env["ANTHROPIC_AUTH_TOKEN"]);
            assert_eq!(
                spec.env,
                crate::routes::gateway::resume_env_for_session(&state, &child).await
            );
            let registered: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sessions WHERE id = $1)")
                    .bind(&child)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert!(!registered, "the binding must precede registration");
        }
    }

    #[tokio::test]
    async fn fork_refuses_an_inaccessible_or_deleted_account() {
        let Some(url) =
            crate::routes::gateway::test_db_url("fork_refuses_an_inaccessible_or_deleted_account")
        else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.unwrap();
        let owner = seed_user(&pool).await;
        let other = seed_user(&pool).await;
        let machine = seed_machine(&pool, owner).await;
        let (account, provider) = seed_account(&pool, other).await;
        sqlx::query(
            "INSERT INTO resource_shares (resource_type, resource_id, grantee_id) \
             SELECT 'account', account_id, $2 FROM account_providers WHERE id = $1",
        )
        .bind(provider)
        .bind(owner)
        .execute(&pool)
        .await
        .unwrap();
        let parent = seed_session(&pool, owner, machine).await;
        let state = AppState::for_test(pool.clone());
        crate::routes::gateway::mint_session_env(
            &state,
            owner,
            &account,
            Family::Anthropic,
            &parent,
        )
        .await
        .unwrap();
        sqlx::query("UPDATE resource_shares SET revoked_at = now() WHERE grantee_id = $1")
            .bind(owner)
            .execute(&pool)
            .await
            .unwrap();
        let mut rx = daemon(&state, machine);
        assert_eq!(fork(&state, caller(owner, SPAWNER), &parent).await, Err(StatusCode::CONFLICT));
        assert!(rx.try_recv().is_err(), "never dispatch without the account");
        sqlx::query("DELETE FROM accounts WHERE name = $1 AND user_id = $2")
            .bind(&account)
            .bind(other)
            .execute(&state.pool)
            .await
            .unwrap();
        assert_eq!(fork(&state, caller(owner, SPAWNER), &parent).await, Err(StatusCode::CONFLICT));
        assert!(rx.try_recv().is_err(), "a deleted account must not become an unbound fork");
    }

    #[tokio::test]
    async fn fork_and_resume_need_dispatch_and_a_machine_the_caller_may_spawn_on() {
        let Some(url) = crate::routes::gateway::test_db_url(
            "fork_and_resume_need_dispatch_and_a_machine_the_caller_may_spawn_on",
        ) else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
        let owner = seed_user(&pool).await;
        let grantee = seed_user(&pool).await;
        let reader = seed_user(&pool).await;
        let stranger = seed_user(&pool).await;
        let machine = seed_machine(&pool, owner).await;
        share(&pool, machine, grantee, "spawn").await;
        share(&pool, machine, reader, "use").await;
        let state = AppState::for_test(pool.clone());
        let sid = seed_session(&pool, owner, machine).await;

        let read_only = caller(owner, READ_ONLY);
        assert_eq!(fork(&state, read_only.clone(), &sid).await, Err(StatusCode::FORBIDDEN));
        assert_eq!(resume(&state, read_only, &sid).await, Err(StatusCode::FORBIDDEN));

        let gate = |user, scopes| {
            let state = state.clone();
            async move { code(super::authorize_launch(&state, &caller(user, scopes), machine).await) }
        };
        assert_eq!(gate(owner, SPAWNER).await, Ok(()));
        assert_eq!(gate(Uuid::nil(), ADMIN).await, Ok(()));
        assert_eq!(gate(grantee, SPAWNER).await, Ok(()));
        assert_eq!(gate(reader, SPAWNER).await, Err(StatusCode::FORBIDDEN));
        assert_eq!(gate(stranger, SPAWNER).await, Err(StatusCode::FORBIDDEN));
        assert_eq!(gate(owner, READ_ONLY).await, Err(StatusCode::FORBIDDEN));

        let _daemon = daemon(&state, machine);
        let child = fork(&state, caller(owner, SPAWNER), &sid)
            .await
            .expect("the owner still forks")
            .expect("claude pre-mints the child id");
        let intent: Option<Uuid> =
            sqlx::query_scalar("SELECT user_id FROM session_spawner_intents WHERE spawn_key = $1")
                .bind(&child)
                .fetch_optional(&pool)
                .await
                .unwrap();
        assert_eq!(intent, Some(owner), "the fork is attributed to its caller");

        assert_eq!(spawned_by(&pool, &sid).await, None);
        assert_eq!(resume(&state, caller(owner, SPAWNER), &sid).await, Ok(()));
        assert_eq!(spawned_by(&pool, &sid).await, Some(owner), "a resume fills a missing spawner");
    }

    #[tokio::test]
    async fn a_revival_never_rewrites_who_spawned_the_session() {
        let Some(url) =
            crate::routes::gateway::test_db_url("a_revival_never_rewrites_who_spawned_the_session")
        else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
        let owner = seed_user(&pool).await;
        let grantee = seed_user(&pool).await;
        let machine = seed_machine(&pool, owner).await;
        let sid = seed_session(&pool, owner, machine).await;
        super::fill_spawner(&pool, &sid, grantee).await;
        super::fill_spawner(&pool, &sid, owner).await;
        assert_eq!(spawned_by(&pool, &sid).await, Some(grantee));
    }

    #[tokio::test]
    async fn a_grantee_run_session_stays_on_the_grantee_account() {
        let Some(url) = crate::routes::gateway::test_db_url(
            "a_grantee_run_session_stays_on_the_grantee_account",
        ) else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
        let owner = seed_user(&pool).await;
        let grantee = seed_user(&pool).await;
        let machine = seed_machine(&pool, owner).await;
        share(&pool, machine, grantee, "spawn").await;
        let (account, provider) = seed_account(&pool, grantee).await;
        let state = AppState::for_test(pool.clone());
        let sid = seed_grantee_run(&state, owner, grantee, machine, &account).await;

        assert_eq!(crate::routes::gateway::acting_user(&state, &sid, None).await, Some(grantee));
        let env = crate::routes::gateway::resume_env_for_session(&state, &sid).await;
        assert!(env.contains_key("ANTHROPIC_AUTH_TOKEN"), "the re-mint was refused: {env:?}");
        let live: Vec<(Uuid, Option<Uuid>)> = sqlx::query_as(
            "SELECT account_id, user_id FROM session_tokens \
             WHERE session_id = $1 AND revoked_at IS NULL",
        )
        .bind(&sid)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(live, vec![(provider, Some(grantee))]);
        assert_eq!(crate::authz::session_owner(&sid, &pool).await.unwrap(), Some(owner));

        let own = seed_session(&pool, owner, machine).await;
        assert_eq!(
            crate::routes::gateway::acting_user(&state, &own, None).await,
            Some(owner),
            "an unbound session still resolves to its owner"
        );
    }

    async fn switch(
        state: &AppState,
        ctx: crate::auth::AuthContext,
        sid: &str,
        account: &str,
    ) -> Result<(), StatusCode> {
        code(
            crate::routes::sessions::switch_account(
                State(state.clone()),
                Extension(ctx),
                Path(sid.to_owned()),
                Json(crate::routes::sessions::SwitchAccountRequest {
                    account: account.to_owned(),
                    family: None,
                }),
            )
            .await,
        )
    }

    async fn binding(pool: &sqlx::PgPool, sid: &str) -> (Uuid, Option<Uuid>) {
        sqlx::query_as(
            "SELECT account_id, user_id FROM session_tokens \
             WHERE session_id = $1 AND revoked_at IS NULL",
        )
        .bind(sid)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn the_machine_owner_never_reaches_the_grantees_other_accounts() {
        let Some(url) = crate::routes::gateway::test_db_url(
            "the_machine_owner_never_reaches_the_grantees_other_accounts",
        ) else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
        let owner = seed_user(&pool).await;
        let grantee = seed_user(&pool).await;
        let machine = seed_machine(&pool, owner).await;
        share(&pool, machine, grantee, "spawn").await;
        let (bound, _) = seed_account(&pool, grantee).await;
        let (other, other_provider) = seed_account(&pool, grantee).await;
        let (mine, my_provider) = seed_account(&pool, owner).await;
        let state = AppState::for_test(pool.clone());
        let sid = seed_grantee_run(&state, owner, grantee, machine, &bound).await;
        let as_owner = caller(owner, SPAWNER);

        assert_eq!(fork(&state, as_owner.clone(), &sid).await, Err(StatusCode::FORBIDDEN));
        let refuse = |ctx: crate::auth::AuthContext| {
            let state = state.clone();
            let sid = sid.clone();
            async move { code(super::refuse_foreign_account(&state, &ctx, &sid).await) }
        };
        assert_eq!(refuse(as_owner.clone()).await, Err(StatusCode::FORBIDDEN));
        assert_eq!(refuse(caller(Uuid::nil(), ADMIN)).await, Ok(()));
        assert_eq!(refuse(caller(grantee, SPAWNER)).await, Ok(()));

        let not_found = Err(StatusCode::NOT_FOUND);
        assert_eq!(switch(&state, as_owner.clone(), &sid, &other).await, not_found);
        assert_eq!(switch(&state, caller(Uuid::nil(), ADMIN), &sid, &other).await, Ok(()));
        assert_eq!(
            binding(&pool, &sid).await,
            (other_provider, Some(grantee)),
            "an admin keeps the runner"
        );

        assert_eq!(switch(&state, as_owner.clone(), &sid, &mine).await, Ok(()));
        assert_eq!(binding(&pool, &sid).await, (my_provider, Some(owner)), "the owner took over");
        assert_eq!(switch(&state, as_owner, &sid, &other).await, not_found);
    }
}
