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

use crate::ports::SessionsBlobs;
use axum::extract::{Multipart, Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};

use cctui_proto::adapter::{AdapterId, BootstrapUploads, SessionSpec};
use cctui_proto::api::{
    ApiError, LaunchRequest, ScheduleLaunchRequest, SpawnRequest, SpawnResponse,
};
use uuid::Uuid;

use cctui_server_kernel::dispatch_types::spawn_ids;
use cctui_telemetry::db::traced;
use tracing::Instrument;

use crate::auth::AuthContext;
use crate::context::SessionsContext;
use crate::error::AppError;
use crate::registry::MachineCommand;
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
/// (never logged; a draft or queued row keeps them sealed) and the daemon
/// injects them into the worker process.
#[tracing::instrument(skip_all, fields(user_id = %ctx.user_id))]
pub async fn spawn_session<S: SessionsContext>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
    multipart: Multipart,
) -> Result<(StatusCode, Json<SpawnResponse>), AppError> {
    let parsed = parse_upload_multipart(multipart, state.cached_upload_caps())
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

    let queueable = (req.clone(), copy_uploads(&parsed.raw));
    let failure = match try_dispatch_spawn(&state, &ctx, req, uploads, parsed.raw).await {
        Ok(launched) => return Ok(launched),
        Err(failure) => failure,
    };
    let (code, Json(ApiError { error })) = failure.response;
    let Some(target) = failure.queue else {
        return Err(AppError::new(code, error));
    };
    let (req, raw) = queueable;
    queue_spawn(&state, &ctx, target, &req, &raw).await.map_err(|e| {
        tracing::warn!(machine = %req.machine_id, "queueing the undelivered spawn: {e}");
        AppError::new(code, error)
    })
}

fn copy_uploads(raw: &[crate::uploads::RawUpload]) -> Vec<crate::uploads::RawUpload> {
    raw.iter()
        .map(|u| crate::uploads::RawUpload {
            name: u.name.clone(),
            bytes: u.bytes.clone(),
            content_type: u.content_type.clone(),
        })
        .collect()
}

/// Where an undelivered spawn may be queued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueTarget {
    pub machine_uuid: Uuid,
    pub owner: Uuid,
}

/// A failed spawn. `queue` is set only when nothing reached the daemon, the
/// one failure that is safe to retry.
pub struct SpawnFailure {
    pub response: (StatusCode, Json<ApiError>),
    pub queue: Option<QueueTarget>,
}

impl SpawnFailure {
    #[must_use]
    pub fn message(&self) -> &str {
        &self.response.1.error
    }
}

impl From<(StatusCode, Json<ApiError>)> for SpawnFailure {
    fn from(response: (StatusCode, Json<ApiError>)) -> Self {
        Self { response, queue: None }
    }
}

impl From<AppError> for SpawnFailure {
    fn from(e: AppError) -> Self {
        e.into_parts().into()
    }
}

/// Whether `error` proves the spawn never reached a daemon.
#[must_use]
pub const fn queues_on(error: &cctui_server_kernel::dispatch_types::LaunchError) -> bool {
    use crate::bus::BusError;
    matches!(
        error,
        cctui_server_kernel::dispatch_types::LaunchError::Delivery(
            BusError::NoDaemon(_) | BusError::Closed
        )
    )
}

/// Store an undelivered spawn as a `queued` session and put it in its inbox
/// lane, which launches it once the machine's daemon is back.
pub async fn queue_spawn<S: SessionsContext>(
    state: &S,
    ctx: &AuthContext,
    target: QueueTarget,
    req: &SpawnRequest,
    uploads: &[crate::uploads::RawUpload],
) -> Result<(StatusCode, Json<SpawnResponse>), AppError> {
    let queued_id =
        store_spawn_row(state, req, target.machine_uuid, uploads, SessionRowStatus::Queued).await?;
    let key = queued_id.to_string();
    crate::inbox::queued::enqueue(state.pool(), target.owner, ctx.user_id, &key).await?;
    tracing::info!(machine = %req.machine_id, queued = %key, "spawn queued until the daemon is back");
    Ok((
        StatusCode::ACCEPTED,
        Json(SpawnResponse {
            command_id: queued_id,
            status: SessionRowStatus::Queued.as_str().into(),
            account: None,
            session_id: None,
        }),
    ))
}

/// Dispatch a spawn to the targeted daemon.
///
/// Shared by the immediate spawn path
/// and the draft-launch path so account env is minted + the command
/// dispatched identically. Validates the `dispatch` scope, env keys and machine
/// access, mints any account gateway env, and pushes `AdapterCommand::Spawn`
/// over the WS.
#[tracing::instrument(skip_all, fields(user_id = %ctx.user_id, machine = %req.machine_id))]
pub async fn dispatch_spawn<S: SessionsContext>(
    state: &S,
    ctx: &AuthContext,
    req: SpawnRequest,
    uploads: Vec<cctui_proto::adapter::BootstrapFile>,
    raw_uploads: Vec<crate::uploads::RawUpload>,
) -> Result<(StatusCode, Json<SpawnResponse>), (StatusCode, Json<ApiError>)> {
    try_dispatch_spawn(state, ctx, req, uploads, raw_uploads).await.map_err(|f| f.response)
}

/// [`dispatch_spawn`], telling an undelivered spawn apart from every other failure.
pub async fn try_dispatch_spawn<S: SessionsContext>(
    state: &S,
    ctx: &AuthContext,
    req: SpawnRequest,
    uploads: Vec<cctui_proto::adapter::BootstrapFile>,
    raw_uploads: Vec<crate::uploads::RawUpload>,
) -> Result<(StatusCode, Json<SpawnResponse>), SpawnFailure> {
    try_dispatch_spawn_as(state, ctx, req, uploads, raw_uploads, Uuid::new_v4()).await
}

/// [`try_dispatch_spawn`] under a caller-chosen `command_id`, the key its
/// `machine_launches` row is recorded under.
pub async fn try_dispatch_spawn_as<S: SessionsContext>(
    state: &S,
    ctx: &AuthContext,
    req: SpawnRequest,
    uploads: Vec<cctui_proto::adapter::BootstrapFile>,
    raw_uploads: Vec<crate::uploads::RawUpload>,
    command_id: Uuid,
) -> Result<(StatusCode, Json<SpawnResponse>), SpawnFailure> {
    require_dispatch(ctx)?;
    let target = validate_spawn(state, ctx, &req, command_id).await?;
    let bound = resolve_spawn_account(state, &target, &req).await?;
    execute_spawn(state, target, bound, &req, uploads, raw_uploads).await
}

/// Starting a run needs the `dispatch` scope: a read-only key may watch, not launch.
fn require_dispatch(ctx: &AuthContext) -> Result<(), (StatusCode, Json<ApiError>)> {
    ctx.requires(crate::auth::Scope::Dispatch).map_err(|code| {
        (code, Json(ApiError { error: "spawning needs the dispatch scope".into() }))
    })
}

use cctui_server_kernel::dispatch_types::BoundAccount;

#[tracing::instrument(skip_all, fields(machine_id = %target.machine_uuid))]
async fn resolve_spawn_account<S: SessionsContext>(
    state: &S,
    target: &SpawnTarget,
    req: &SpawnRequest,
) -> Result<BoundAccount, (StatusCode, Json<ApiError>)> {
    state
        .resolve_spawn_account(
            &cctui_server_kernel::dispatch_types::AccountTarget {
                uid: target.uid,
                adapter_id: target.adapter_id.clone(),
                token_session_id: target.token_session_id.clone(),
            },
            req,
        )
        .await
}

/// The validated spawn target plus the ids minted for it.
pub struct SpawnTarget {
    pub acting_user_id: Uuid,
    pub machine_uuid: Uuid,
    pub owner: Uuid,
    /// The user whose accounts the spawn resolves against.
    pub uid: Uuid,
    pub adapter_id: String,
    pub command_id: Uuid,
    /// For claude-code the session id is pre-minted here and handed to the
    /// worker as `--session-id` (mirroring the fork path), so the gateway token
    /// can be bound to the *real* session id the worker registers as. Only
    /// claude registers under it, so only claude gets it back in the response.
    pub pre_session_id: Option<Uuid>,
    /// The key the gateway session token, spawn capability and spawn intents
    /// are stored under: the pre-minted session id for claude, else the
    /// `command_id`. Always sent to the daemon as `Spawn.session_id`, whose
    /// adapters echo it as `spawn_key` on `SessionStarted` so the server can
    /// re-key all of it onto the id the harness really registered under.
    pub token_key: Uuid,
    /// `token_key` as the text id the stores use.
    pub token_session_id: String,
    /// The context items this spawn resolved, server-side, from the request's
    /// picks, the profile's pinned set and the scope-matching memories.
    pub context: Vec<crate::routes::context::ContextItem>,
}

#[tracing::instrument(skip_all)]
pub async fn validate_spawn<S: SessionsContext>(
    state: &S,
    ctx: &AuthContext,
    req: &SpawnRequest,
    command_id: Uuid,
) -> Result<SpawnTarget, (StatusCode, Json<ApiError>)> {
    state.validate_env(req)?;

    let (machine_uuid, owner) = state.resolve_spawn_machine(ctx, &req.machine_id).await?;
    refuse_missing_cwd(state, machine_uuid, &req.working_dir).await?;

    let adapter_id = state.spawn_adapter_on(machine_uuid, req.adapter_id.as_deref()).await?;
    let (pre_session_id, token_key) = spawn_ids(&adapter_id, command_id);
    let token_session_id = token_key.to_string();
    // Every intent is written before this returns: a fast daemon's
    // `SessionStarted` consumes them as soon as the spawn is delivered.
    let key = token_session_id.as_str();
    let (context, (), (), ()) = tokio::join!(
        remember_context(state, ctx, req, key, owner, machine_uuid, &adapter_id),
        async {
            if req.auto_archive {
                crate::auto_archive::remember_intent(state, key).await;
            }
        },
        state.remember_spawner(key, ctx.user_id, req.name.as_deref()),
        async {
            if state.config().features.contains(cctui_proto::features::Feature::Automation) {
                let profile_owner = ctx.owner_filter().unwrap_or(owner);
                tokio::join!(
                    crate::followup::remember_intent(state.pool(), key, req),
                    crate::auto_followup::remember_intent(state.pool(), key, req, profile_owner),
                );
            }
        },
    );
    // Accounts are user-owned. The admin token has no user identity, so it
    // resolves the account against the target machine's owner —
    // the session runs on that user's machine with that user's account.
    let uid = ctx.owner_filter().unwrap_or(owner);

    Ok(SpawnTarget {
        acting_user_id: ctx.user_id,
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

/// A working directory the machine's daemon says is absent fails here, before
/// any intent or session row exists. An unreachable daemon is not a refusal.
pub async fn refuse_missing_cwd<S: SessionsContext>(
    state: &S,
    machine_uuid: Uuid,
    working_dir: &str,
) -> Result<(), (StatusCode, Json<ApiError>)> {
    let cwd = working_dir.trim();
    if cwd.is_empty() || !crate::routes::fs::dir_missing(state, machine_uuid, cwd).await {
        return Ok(());
    }
    let error = format!("working directory {cwd} does not exist on that machine");
    Err((StatusCode::BAD_REQUEST, Json(ApiError { error })))
}

const fn requested_labels(
    features: cctui_proto::features::FeatureSet,
    ids: &[String],
) -> &[String] {
    if features.contains(cctui_proto::features::Feature::Organising) { ids } else { &[] }
}

/// The spawn's labels and context set, recorded under `spawn_key`.
async fn remember_context<S: SessionsContext>(
    state: &S,
    ctx: &AuthContext,
    req: &SpawnRequest,
    spawn_key: &str,
    owner: Uuid,
    machine_uuid: Uuid,
    adapter_id: &str,
) -> Vec<crate::routes::context::ContextItem> {
    // In the machine owner's vocabulary: an admin's picker holds labels of
    // every user, and the session belongs to the owner.
    let requested = requested_labels(state.config().features, &req.label_ids);
    let label_ids =
        crate::spawn_labels::for_spawn(state.pool(), requested, ctx.owner_filter(), owner).await;
    let scope = launch_owner_scope(ctx, req, &label_ids, owner, machine_uuid, adapter_id);
    if !state.config().features.contains(cctui_proto::features::Feature::Context) {
        tokio::join!(
            crate::spawn_labels::remember_intent(state.pool(), spawn_key, &label_ids),
            crate::routes::context::ownership::record(state.pool(), spawn_key, &scope),
        );
        return Vec::new();
    }
    let ((), context) = tokio::join!(
        crate::spawn_labels::remember_intent(state.pool(), spawn_key, &label_ids),
        resolve_spawn_context(state, req, &scope),
    );
    crate::routes::context::remember_intent(state.pool(), spawn_key, &context, &scope).await;
    context
}

/// The owner and coordinates a spawn is recorded under. The resolved machine
/// uuid, not `req.machine_id`, which may be a name: a machine filter must
/// resolve the same here and on the child path.
fn launch_owner_scope(
    ctx: &AuthContext,
    req: &SpawnRequest,
    label_ids: &[String],
    owner: Uuid,
    machine_uuid: Uuid,
    adapter_id: &str,
) -> crate::routes::context::StoredScope {
    crate::routes::context::StoredScope {
        user_id: ctx.owner_filter().unwrap_or(owner),
        coords: crate::routes::context::SpawnScope {
            machine_id: Some(machine_uuid.to_string()),
            working_dir: Some(req.working_dir.clone()),
            label_ids: label_ids.to_vec(),
            adapter: Some(adapter_id.to_owned()),
        },
        dispatched: false,
    }
}

/// Resolve the reusable context for this spawn: the matching rules, the
/// request's explicit picks, the profile's pinned items and — when the request
/// asks for `auto` — every enabled memory whose filter matches the spawn's
/// machine, working dir, labels and adapter.
///
/// Best-effort: a lookup failure launches the session without context rather
/// than refusing it. Context is a convenience, never a precondition.
#[tracing::instrument(
    target = "otel",
    skip_all,
    fields(code.function.name = "resolve_spawn_context")
)]
async fn resolve_spawn_context<S: SessionsContext>(
    state: &S,
    req: &SpawnRequest,
    scope: &crate::routes::context::StoredScope,
) -> Vec<crate::routes::context::ContextItem> {
    let uid = scope.user_id;
    let asked = req.context.clone().unwrap_or_default();
    let items = match crate::routes::context::list_for_user(state.pool(), uid).await {
        Ok(items) => items,
        Err(e) => {
            tracing::warn!(error = %e, "context lookup failed; spawning without it");
            return Vec::new();
        }
    };
    let mut picks = asked.items;
    picks.extend(profile_context_names(state, req, uid, &items).await);
    crate::routes::context::resolve_for_spawn(&items, &picks, &scope.coords)
}

/// The names of the items the request's profile pins. Empty when the request
/// names no profile — the webui also applies a profile by filling the form,
/// and doing both is harmless because picks are unioned by id.
async fn profile_context_names<S: SessionsContext>(
    state: &S,
    req: &SpawnRequest,
    uid: Uuid,
    items: &[crate::routes::context::ContextItem],
) -> Vec<String> {
    let Some(profile_id) = req.profile_id else { return Vec::new() };
    let pinned: Option<Vec<Uuid>> = traced(
        "SELECT context_items FROM session_profiles WHERE id = $1 AND user_id = $2",
        |sql| sqlx::query_scalar(sql).bind(profile_id).bind(uid).fetch_optional(state.pool()),
    )
    .await
    .unwrap_or_default();
    pinned
        .unwrap_or_default()
        .iter()
        .filter_map(|id| items.iter().find(|i| &i.id == id))
        .map(|i| i.name.clone())
        .collect()
}

#[tracing::instrument(target = "otel", skip_all, fields(code.function.name = "stage_uploads"))]
async fn stage_uploads<S: SessionsContext>(
    state: &S,
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
        crate::routes::attachments::record_uploads::<S>(
            state.pool(),
            token_session_id,
            &raw_uploads,
            &[],
        )
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

#[tracing::instrument(
    target = "otel",
    skip_all,
    fields(code.function.name = "spawn_service_tier")
)]
async fn spawn_service_tier<S: SessionsContext>(
    state: &S,
    adapter_id: &str,
    req: &SpawnRequest,
    token_session_id: &str,
) -> Option<String> {
    // Codex's own default tier is `priority`, so an unset tier is the expensive
    // one: resolve to a concrete value here rather than letting the worker
    // inherit whatever the machine's config.toml happens to say.
    if cctui_server_kernel::execution_settings::adapter_takes_service_tier(adapter_id) {
        let account_settings = state.resolve_session_settings(token_session_id).await;
        Some(cctui_server_kernel::execution_settings::resolve_service_tier(
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
    let prompt = templated_prompt(req, context);
    if req.dictated { prompt.map(|p| cctui_proto::injected::dictated_prompt(&p)) } else { prompt }
}

fn templated_prompt(
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

#[tracing::instrument(
    skip_all,
    fields(machine_id = %target.machine_uuid, session_id = %target.token_session_id),
)]
async fn execute_spawn<S: SessionsContext>(
    state: &S,
    target: SpawnTarget,
    bound: BoundAccount,
    req: &SpawnRequest,
    uploads: Vec<cctui_proto::adapter::BootstrapFile>,
    raw_uploads: Vec<crate::uploads::RawUpload>,
) -> Result<(StatusCode, Json<SpawnResponse>), SpawnFailure> {
    let SpawnTarget {
        acting_user_id,
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
        hook_policy: None,
    };
    state.persist_spawn_capability(machine_uuid, req, permission_mode, &token_session_id).await;
    let launched = state
        .launch_durable(
            acting_user_id,
            cctui_server_kernel::dispatch_types::MachineSpawn {
                machine_id: machine_uuid,
                command_id,
                token_key,
                spec,
                failed: crate::commands::FailedSpawnRow {
                    session_id: token_session_id.clone(),
                    machine_id: machine_uuid,
                    user_id: owner,
                    adapter_id: adapter_id.clone(),
                    working_dir: req.working_dir.clone(),
                    name: req.name.clone().filter(|n| !n.trim().is_empty()),
                    model: spec_model,
                    effort: spec_effort,
                },
                route_session: None,
            },
        )
        .instrument(tracing::info_span!(
            target: "otel",
            "launch_durable",
            code.function.name = "launch_durable"
        ))
        .await;
    if let Err(error) = launched {
        if matches!(
            error,
            cctui_server_kernel::dispatch_types::LaunchError::Delivery(_)
                | cctui_server_kernel::dispatch_types::LaunchError::Policy(_)
        ) && let Err(e) =
            crate::routes::attachments::delete_attachments(state.pool(), &recorded_ids).await
        {
            tracing::warn!(session = %token_session_id, "undoing bootstrap attachments: {e}");
        }
        tracing::warn!(machine = %req.machine_id, %command_id, %adapter_id, %error, "spawn not launched");
        let queue = queues_on(&error).then_some(QueueTarget { machine_uuid, owner });
        let no_daemon = no_daemon_wording(state, machine_uuid, &error).await;
        return Err(SpawnFailure { response: launch_failure(error, no_daemon), queue });
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

/// The no-daemon wording for `machine`, read only when `error` needs it.
async fn no_daemon_wording<S: SessionsContext>(
    state: &S,
    machine: Uuid,
    error: &cctui_server_kernel::dispatch_types::LaunchError,
) -> &'static str {
    use cctui_server_kernel::dispatch_types::{DAEMON_OFFLINE, LaunchError};
    match error {
        LaunchError::Delivery(crate::bus::BusError::NoDaemon(_)) => {
            state.no_daemon_detail(machine).await
        }
        _ => DAEMON_OFFLINE,
    }
}

fn launch_failure(
    error: cctui_server_kernel::dispatch_types::LaunchError,
    no_daemon: &'static str,
) -> (StatusCode, Json<ApiError>) {
    use cctui_server_kernel::dispatch_types::LaunchError;
    match error {
        LaunchError::Storage(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError { error: crate::error::DB_ERROR.into() }),
        ),
        LaunchError::Delivery(err) => dispatch_error(&err, no_daemon),
        LaunchError::Policy(detail) => {
            (StatusCode::BAD_REQUEST, Json(ApiError { error: format!("hook policy: {detail}") }))
        }
        LaunchError::Refused(detail) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiError { error: format!("the daemon refused the spawn: {detail}") }),
        ),
    }
}

/// The 503 a spawn answers when its command never reached the daemon.
fn dispatch_error(
    err: &crate::bus::BusError,
    no_daemon: &'static str,
) -> (StatusCode, Json<ApiError>) {
    let error = match err {
        crate::bus::BusError::NoDaemon(_) => no_daemon,
        _ => "daemon disconnected mid-dispatch",
    };
    (StatusCode::SERVICE_UNAVAILABLE, Json(ApiError { error: error.into() }))
}

/// [`authorize_launch`] on the machine `session_id` runs on. A session with no
/// machine still needs the `dispatch` scope.
pub async fn authorize_session_launch<S: SessionsContext>(
    state: &S,
    ctx: &AuthContext,
    session_id: &str,
) -> Result<(), AppError> {
    let machine: Option<Option<Uuid>> =
        traced("SELECT machine_uuid FROM sessions WHERE id = $1", |sql| {
            sqlx::query_scalar(sql).bind(session_id).fetch_optional(state.pool())
        })
        .await?;
    match machine.flatten() {
        Some(machine_uuid) => state.authorize_launch(ctx, machine_uuid).await,
        None => require_dispatch(ctx).map_err(|(code, Json(e))| AppError::new(code, e.error)),
    }
}

/// A fork inherits its parent's account.
///
/// Refused when the parent runs on
/// another user's account (the machine owner forking a spawn grantee's
/// session), so that account only ever serves the session it was bound to.
pub async fn refuse_foreign_account<S: SessionsContext>(
    state: &S,
    ctx: &AuthContext,
    session_id: &str,
) -> Result<(), AppError> {
    if ctx.is_admin() {
        return Ok(());
    }
    match state.acting_user(session_id, None).await {
        Some(runs_for) if runs_for != ctx.user_id => Err(AppError::new(
            StatusCode::FORBIDDEN,
            "this session runs on another user's account and cannot be forked",
        )),
        _ => Ok(()),
    }
}

/// Persist a spawn payload as a `draft` session row, its env sealed.
///
/// No daemon
/// dispatch happens, and the row is excluded from liveness/reaping via its
/// sticky `draft` status. Attached files are kept
/// in the blob store under the draft id so the launch can stage them. Returns
/// the new draft session id in `command_id` with `status = "draft"`.
pub async fn save_draft<S: SessionsContext>(
    state: &S,
    ctx: &AuthContext,
    req: &SpawnRequest,
    uploads: &[crate::uploads::RawUpload],
) -> Result<(StatusCode, Json<SpawnResponse>), AppError> {
    let (machine_uuid, _) = state
        .resolve_owned_machine(ctx, &req.machine_id)
        .await
        .map_err(|(code, Json(e))| AppError::new(code, e.error))?;
    let draft_id =
        store_spawn_row(state, req, machine_uuid, uploads, SessionRowStatus::Draft).await?;
    let draft_key = draft_id.to_string();
    if state.config().features.contains(cctui_proto::features::Feature::Organising) {
        crate::spawn_labels::sync_draft(
            state.pool(),
            &draft_key,
            &req.label_ids,
            ctx.owner_filter(),
        )
        .await;
    }
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

/// Insert a session row holding `req` under `metadata.draft` (its env sealed
/// beside it), with its files in the blob store, for a later launch to rebuild
/// the spawn.
pub async fn store_spawn_row<S: SessionsContext>(
    state: &S,
    req: &SpawnRequest,
    machine_uuid: Uuid,
    uploads: &[crate::uploads::RawUpload],
    status: SessionRowStatus,
) -> Result<Uuid, AppError> {
    let adapter_id = req.adapter_id.clone().unwrap_or_else(|| "claude-code".to_owned());
    let mut payload = req.clone();
    payload.save_draft = false;
    let metadata = draft_metadata(payload, crate::crypto::vault_key)?;

    let id = Uuid::new_v4();
    let name = req.name.as_deref().filter(|n| !n.trim().is_empty());
    let model = req.model.as_deref().filter(|m| !m.trim().is_empty());
    let effort = req.effort.as_deref().filter(|e| !e.trim().is_empty());
    traced(
        r"INSERT INTO sessions
            (id, machine_id, machine_uuid, working_dir, status, registered_at, last_heartbeat,
             metadata, adapter_id, session_name, model, effort)
          VALUES ($1, $2, $3, $4, $10, now(), now(), $5, $6, $7, $8, $9)",
        |sql| {
            sqlx::query(sql)
                .bind(id)
                .bind(&req.machine_id)
                .bind(machine_uuid)
                .bind(&req.working_dir)
                .bind(&metadata)
                .bind(&adapter_id)
                .bind(name)
                .bind(model)
                .bind(effort)
                .bind(status.as_str())
                .execute(state.pool())
        },
    )
    .await?;

    let key = id.to_string();
    if !uploads.is_empty() {
        crate::routes::attachments::record_uploads::<S>(state.pool(), &key, uploads, &[])
            .await
            .map_err(|e| {
                tracing::error!(session = %key, "recording stored spawn attachments: {e}");
                AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "could not store the attachments")
            })?;
    }
    Ok(id)
}

/// No vault key to seal `<redacted>` spans with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NeedsVaultKey;

/// `content` with its `<redacted>` spans masked, plus the real text
/// vault-encrypted when there is a span. `key` is read only then.
pub fn seal_redacted(
    content: &str,
    key: impl FnOnce() -> Vec<u8>,
) -> Result<(String, Option<String>), NeedsVaultKey> {
    if !cctui_crypto::redact::has_user_spans(content) {
        return Ok((content.to_owned(), None));
    }
    let key = key();
    if key.is_empty() {
        return Err(NeedsVaultKey);
    }
    let masked = cctui_crypto::redact::mask_user_spans(content).into_owned();
    Ok((masked, Some(crate::crypto::encrypt(content, &key))))
}

/// Key of the vault-encrypted original prompt beside `metadata.draft`.
pub const DRAFT_SEALED_PROMPT: &str = "draft_sealed_prompt";
/// Key of the vault-encrypted env map beside `metadata.draft`.
pub const DRAFT_SEALED_ENV: &str = "draft_sealed_env";
const DRAFT_SEAL_NEEDS_KEY: &str = "a draft whose prompt has <redacted> spans can only be saved \
     when the server has a vault key (CCTUI_VAULT_KEY)";
pub const DRAFT_UNSEALABLE: &str = "the draft's sealed prompt does not decrypt with the vault key";

/// `{ "draft": payload }` with the prompt's `<redacted>` spans masked; the
/// real prompt is kept only encrypted, under [`DRAFT_SEALED_PROMPT`], and the
/// env values only under [`DRAFT_SEALED_ENV`], the payload naming their keys.
#[allow(clippy::too_long_first_doc_paragraph)]
pub fn draft_metadata(
    mut payload: SpawnRequest,
    key: impl Fn() -> Vec<u8>,
) -> Result<serde_json::Value, AppError> {
    let env = std::mem::take(&mut payload.env);
    crate::sealed_env::bounded(env.len(), &env)
        .map_err(|e| AppError::new(StatusCode::BAD_REQUEST, e))?;
    let sealed_env = crate::sealed_env::seal(&env, &key)
        .map_err(|e| AppError::new(StatusCode::BAD_REQUEST, e.message()))?;
    payload.env_keys = crate::sealed_env::names(&env);
    let sealed = if let Some(prompt) = payload.prompt.as_deref() {
        let (masked, sealed) = seal_redacted(prompt, &key)
            .map_err(|_| AppError::new(StatusCode::BAD_REQUEST, DRAFT_SEAL_NEEDS_KEY))?;
        payload.prompt = Some(masked);
        sealed
    } else {
        None
    };
    let draft = serde_json::to_value(&payload).map_err(|e| {
        tracing::error!("serializing draft payload: {e}");
        AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "could not encode draft")
    })?;
    let mut metadata = serde_json::json!({ "draft": draft });
    if let Some(sealed) = sealed {
        metadata[DRAFT_SEALED_PROMPT] = serde_json::Value::String(sealed);
    }
    if let Some(sealed) = sealed_env {
        metadata[DRAFT_SEALED_ENV] = serde_json::Value::String(sealed);
    }
    Ok(metadata)
}

/// A draft edit's env: `keep` names hold their sealed values, `set` replaces.
pub fn edited_draft_env(
    stored: Option<&str>,
    keep: &[String],
    set: &crate::sealed_env::EnvMap,
    key: impl FnOnce() -> Vec<u8>,
) -> Result<crate::sealed_env::EnvMap, AppError> {
    crate::sealed_env::bounded(keep.len().max(set.len()), set)
        .map_err(|e| AppError::new(StatusCode::BAD_REQUEST, e))?;
    crate::sealed_env::apply_edit(stored, keep, set, key)
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, e.message()))
}

fn unseal_draft_env(req: &mut SpawnRequest, metadata: &serde_json::Value) -> Result<(), AppError> {
    let sealed = metadata.get(DRAFT_SEALED_ENV).and_then(serde_json::Value::as_str);
    req.env = crate::sealed_env::unseal(sealed, crate::crypto::vault_key)
        .map_err(|e| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, e.message()))?;
    Ok(())
}

/// Puts the real prompt back into a draft read from `metadata`.
pub fn unseal_draft_prompt(
    req: &mut SpawnRequest,
    metadata: &serde_json::Value,
    key: impl FnOnce() -> Vec<u8>,
) -> Result<(), AppError> {
    let Some(sealed) = metadata.get(DRAFT_SEALED_PROMPT).and_then(serde_json::Value::as_str) else {
        return Ok(());
    };
    let unsealable = || AppError::new(StatusCode::INTERNAL_SERVER_ERROR, DRAFT_UNSEALABLE);
    let key = key();
    if key.is_empty() {
        return Err(unsealable());
    }
    req.prompt = Some(crate::crypto::decrypt(sealed, &key).ok_or_else(unsealable)?);
    Ok(())
}

/// `POST /api/v1/sessions/{id}/launch`.
///
/// Promote a draft to a live
/// spawn: read the stored payload, lay the launch env over its sealed env, dispatch the
/// real spawn (minting account gateway env), then delete the draft row. The
/// live session appears via the daemon's normal registration.
#[tracing::instrument(skip_all, fields(session_id = %session_id))]
pub async fn launch_draft<S: SessionsContext>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
    Path(session_id): Path<String>,
    Json(launch): Json<LaunchRequest>,
) -> Result<(StatusCode, Json<SpawnResponse>), AppError> {
    launch_stored_draft(&state, &ctx, &session_id, launch.env).await
}

/// Shared with the scheduled-launch sweep: a due draft must go out through the
/// same path as a hand-launched one.
#[tracing::instrument(skip_all, fields(session_id = %session_id))]
pub async fn launch_stored_draft<S: SessionsContext>(
    state: &S,
    ctx: &AuthContext,
    session_id: &str,
    env: std::collections::BTreeMap<String, String>,
) -> Result<(StatusCode, Json<SpawnResponse>), AppError> {
    let (mut req, uploads, raw_uploads) =
        stored_spawn(state, session_id, SessionRowStatus::Draft).await?;
    // Account gateway env is minted in dispatch.
    req.env = crate::sealed_env::overlay(std::mem::take(&mut req.env), env);
    req.save_draft = false;
    // Labels put on the draft card after it was saved travel with the launch.
    for id in crate::spawn_labels::draft_label_ids(state.pool(), session_id).await {
        if !req.label_ids.contains(&id) {
            req.label_ids.push(id);
        }
    }

    let outcome = dispatch_spawn(state, ctx, req, uploads, raw_uploads)
        .await
        .map_err(|(code, Json(e))| AppError::new(code, e.error))?;

    // Drop the draft only after a successful dispatch; the live session is born
    // from the daemon's registration with its own id. The `sessions` delete
    // trigger drops the draft's attachment rows with it.
    let deleted = traced("DELETE FROM sessions WHERE id = $1 AND status = 'draft'", |sql| {
        sqlx::query(sql).bind(session_id).execute(state.pool())
    })
    .await;
    if let Err(e) = deleted {
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

type StoredSpawn =
    (SpawnRequest, Vec<cctui_proto::adapter::BootstrapFile>, Vec<crate::uploads::RawUpload>);

/// The spawn a `draft` or `queued` row stores, with its prompt and env unsealed
/// and its files read back from the blob store.
pub async fn stored_spawn<S: SessionsContext>(
    state: &S,
    session_id: &str,
    expect: SessionRowStatus,
) -> Result<StoredSpawn, AppError> {
    let noun = if expect == SessionRowStatus::Draft { "draft" } else { "queued spawn" };
    let row: Option<(String, serde_json::Value)> =
        traced("SELECT status, metadata FROM sessions WHERE id = $1", |sql| {
            sqlx::query_as(sql).bind(session_id).fetch_optional(state.pool())
        })
        .await?;
    let Some((status, metadata)) = row else {
        return Err(AppError::new(StatusCode::NOT_FOUND, format!("{noun} not found")));
    };
    if SessionRowStatus::parse(&status) != Some(expect) {
        return Err(AppError::new(StatusCode::BAD_REQUEST, format!("session is not a {noun}")));
    }
    let mut req: SpawnRequest = metadata
        .get("draft")
        .cloned()
        .ok_or_else(|| {
            AppError::new(StatusCode::BAD_REQUEST, format!("{noun} row missing payload"))
        })
        .and_then(|v| {
            serde_json::from_value(v).map_err(|e| {
                AppError::new(StatusCode::BAD_REQUEST, format!("corrupt {noun} payload: {e}"))
            })
        })?;
    unseal_draft_prompt(&mut req, &metadata, crate::crypto::vault_key)?;
    unseal_draft_env(&mut req, &metadata)?;
    req.env_keys.clear();
    let (uploads, raw_uploads) =
        load_draft_uploads::<S>(state.pool(), session_id, &req.attachment_names).await?;
    req.attachment_names.clear();
    Ok((req, uploads, raw_uploads))
}

/// Launch a `queued` spawn as `ctx`, then drop its row: the live session is
/// born from the daemon's registration, as for a draft.
pub async fn launch_queued<S: SessionsContext>(
    state: &S,
    ctx: &AuthContext,
    session_id: &str,
) -> Result<SpawnResponse, SpawnFailure> {
    let (req, uploads, raw_uploads) =
        stored_spawn(state, session_id, SessionRowStatus::Queued).await?;
    let (_, Json(launched)) = try_dispatch_spawn(state, ctx, req, uploads, raw_uploads).await?;
    let deleted = traced("DELETE FROM sessions WHERE id = $1 AND status = 'queued'", |sql| {
        sqlx::query(sql).bind(session_id).execute(state.pool())
    })
    .await;
    if let Err(e) = deleted {
        tracing::warn!(%session_id, "queued spawn launched but row delete failed: {e}");
    }
    tracing::info!(queued = %session_id, command_id = %launched.command_id, "queued spawn launched");
    Ok(launched)
}

/// The draft's stored files, re-read from the blob store in the shape
/// [`dispatch_spawn`] takes, so the launched session stages and records them
/// exactly like a direct spawn. `expected` are the names the draft payload
/// references: any of them absent from the store fails the launch rather than
/// spawning a prompt that points at files the worker never gets.
#[allow(clippy::too_long_first_doc_paragraph)]
pub async fn load_draft_uploads<B: SessionsBlobs>(
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
        let Some((_, bytes)) = B::fetch_session_blob(pool, draft_id, &row.hash).await? else {
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
pub async fn put_draft_attachments<S: SessionsContext>(
    State(state): State<S>,
    Path(session_id): Path<String>,
    multipart: Multipart,
) -> Result<Json<Vec<crate::routes::attachments::SessionAttachment>>, AppError> {
    let parsed = parse_upload_multipart(multipart, state.cached_upload_caps())
        .await
        .map_err(|(code, Json(e))| AppError::new(code, e.error))?;
    let status: Option<String> = traced("SELECT status FROM sessions WHERE id = $1", |sql| {
        sqlx::query_scalar(sql).bind(&session_id).fetch_optional(state.pool())
    })
    .await?;
    let Some(status) = status else {
        return Err(AppError::new(StatusCode::NOT_FOUND, "draft not found"));
    };
    if SessionRowStatus::parse(&status) != Some(SessionRowStatus::Draft) {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "session is not a draft"));
    }
    let rows = replace_draft_attachments::<S>(state.pool(), &session_id, &parsed.raw)
        .await
        .map_err(|e| {
            tracing::error!(draft = %session_id, "replacing draft attachments: {e}");
            AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "could not store the attachments")
        })?;
    tracing::info!(draft = %session_id, files = rows.len(), "draft attachments replaced");
    Ok(Json(rows))
}

/// Swap a draft's attachment rows for `uploads` in one transaction. Blobs are
/// content-addressed and shared, so only the rows go.
pub async fn replace_draft_attachments<B: SessionsBlobs>(
    pool: &sqlx::PgPool,
    draft_id: &str,
    uploads: &[crate::uploads::RawUpload],
) -> Result<Vec<crate::routes::attachments::SessionAttachment>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let conn = &mut *tx;
    traced("DELETE FROM session_attachments WHERE session_id = $1", move |sql| {
        sqlx::query(sql).bind(draft_id).execute(conn)
    })
    .await?;
    let rows =
        crate::routes::attachments::record_uploads_in::<B>(&mut tx, draft_id, uploads, &[]).await?;
    tx.commit().await?;
    Ok(rows)
}

/// `POST /api/v1/sessions/{id}/schedule-launch`. Queue a draft to launch at
/// `launch_at`; replaces any schedule already on it.
pub async fn schedule_draft_launch<S: SessionsContext>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
    Path(session_id): Path<String>,
    Json(body): Json<ScheduleLaunchRequest>,
) -> Result<StatusCode, AppError> {
    require_dispatch(&ctx).map_err(|(code, Json(e))| AppError::new(code, e.error))?;
    let at = crate::scheduled_messages::parse_deliver_at(&body.launch_at, chrono::Utc::now())
        .map_err(|e| AppError::new(StatusCode::BAD_REQUEST, e.to_string()))?;
    let status: Option<String> = traced("SELECT status FROM sessions WHERE id = $1", |sql| {
        sqlx::query_scalar(sql).bind(&session_id).fetch_optional(state.pool())
    })
    .await?;
    let Some(status) = status else {
        return Err(AppError::new(StatusCode::NOT_FOUND, "draft not found"));
    };
    if SessionRowStatus::parse(&status) != Some(SessionRowStatus::Draft) {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "session is not a draft"));
    }
    crate::scheduled_spawns::schedule(
        state.pool(),
        state.bus(),
        &session_id,
        Some(ctx.user_id),
        at,
    )
    .await?;
    tracing::info!(draft = %session_id, launch_at = %at, "draft launch scheduled");
    Ok(StatusCode::ACCEPTED)
}

/// `POST /api/v1/sessions/{id}/cancel-launch`. Drop a draft's schedule and
/// leave the draft itself alone.
pub async fn cancel_draft_launch<S: SessionsContext>(
    State(state): State<S>,
    Path(session_id): Path<String>,
) -> Result<StatusCode, AppError> {
    let scheduling = state.config().features.contains(cctui_proto::features::Feature::Scheduling);
    if !scheduling
        || !crate::scheduled_spawns::cancel(state.pool(), state.bus(), &session_id).await?
    {
        return Err(AppError::new(StatusCode::NOT_FOUND, "draft is not scheduled"));
    }
    tracing::info!(draft = %session_id, "draft launch schedule cancelled");
    Ok(StatusCode::NO_CONTENT)
}

const DISCARD_SQL: &str = "DELETE FROM sessions WHERE id = $1 AND status IN ('draft', 'queued')";

/// `POST /api/v1/sessions/{id}/discard`. Delete a draft, or cancel a queued
/// spawn. Only acts on those rows so it can never delete a real session.
pub async fn discard_draft<S: SessionsContext>(
    State(state): State<S>,
    Path(session_id): Path<String>,
) -> Result<StatusCode, AppError> {
    crate::inbox::queued::cancel(state.pool(), &session_id).await?;
    let res =
        traced(DISCARD_SQL, |sql| sqlx::query(sql).bind(&session_id).execute(state.pool())).await?;
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
pub async fn stage_session_files<S: SessionsContext>(
    State(state): State<S>,
    Path(session_id): Path<String>,
    multipart: Multipart,
) -> Result<Json<cctui_proto::api::StageFilesResponse>, AppError> {
    let parsed = parse_upload_multipart(multipart, state.cached_upload_caps())
        .await
        .map_err(|(code, Json(e))| AppError::new(code, e.error))?;
    if parsed.files.is_empty() {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "no files in upload"));
    }
    let count = parsed.files.len();
    // Same ordering as the spawn path: store the blobs first so a blob-store
    // failure fails the request before any file reaches the machine.
    let recorded = crate::routes::attachments::record_uploads::<S>(
        state.pool(),
        &session_id,
        &parsed.raw,
        &[],
    )
    .await
    .map_err(|e| {
        tracing::error!(%session_id, "recording attachments: {e}");
        AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "could not store the attachments")
    })?;
    let recorded_ids: Vec<Uuid> = recorded.iter().map(|a| a.id).collect();

    let staged = crate::bus::stage_files(&state, &session_id, parsed.files).await;
    if staged.is_err()
        && let Err(e) =
            crate::routes::attachments::delete_attachments(state.pool(), &recorded_ids).await
    {
        tracing::warn!(%session_id, "undoing mid-chat attachments: {e}");
    }
    match staged {
        Ok(paths) => {
            tracing::info!(%session_id, count, "staged mid-chat files");
            let names: Vec<String> =
                paths.iter().map(|p| p.rsplit('/').next().unwrap_or(p).to_owned()).collect();
            if let Err(e) =
                crate::routes::attachments::set_attachment_names(state.pool(), &recorded, &names)
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
pub async fn get_machine_commands<S: SessionsContext>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
    Path(machine_id): Path<String>,
) -> Result<Json<Vec<MachineCommand>>, AppError> {
    let owned = match Uuid::parse_str(&machine_id) {
        Ok(id) => {
            traced("SELECT EXISTS (SELECT 1 FROM machines WHERE id = $1 AND user_id = $2)", |sql| {
                sqlx::query_scalar::<_, bool>(sql)
                    .bind(id)
                    .bind(ctx.user_id)
                    .fetch_one(state.pool())
            })
            .await?
        }
        Err(_) => false,
    };
    if !owned {
        return Err(AppError::new(StatusCode::NOT_FOUND, "machine not found"));
    }
    let commands = {
        let mut registry = state.registry().write().await;
        registry.take_machine_commands(&machine_id)
    };
    Ok(Json(commands))
}

#[cfg(test)]
mod tests {

    #[test]
    fn spawn_labels_are_dropped_with_organising_off() {
        use cctui_proto::features::{Feature, FeatureSet};
        let ids = vec!["11111111-1111-4111-8111-111111111111".to_owned()];
        assert_eq!(super::requested_labels(FeatureSet::all(), &ids), ids.as_slice());
        let off = FeatureSet::all().without(Feature::Organising);
        assert!(super::requested_labels(off, &ids).is_empty());
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
}

#[cfg(test)]
mod draft_seal_tests {

    use cctui_proto::api::SpawnRequest;

    use super::{
        DRAFT_SEALED_PROMPT, NeedsVaultKey, draft_metadata, seal_redacted, unseal_draft_prompt,
    };

    const TAGGED: &str = "log in with <redacted>hunter2-xyz</redacted> please";
    const MASKED: &str = "log in with [REDACTED:user] please";

    fn request(machine: &str, prompt: &str) -> SpawnRequest {
        serde_json::from_value(serde_json::json!({
            "machine_id": machine, "working_dir": "/w", "prompt": prompt,
        }))
        .unwrap()
    }

    #[test]
    fn a_dictated_spawn_prompt_is_marked_and_a_typed_one_is_not() {
        let mut req = request("m", "fix the login bug");
        assert_eq!(super::spawn_prompt(&req, &[]).as_deref(), Some("fix the login bug"));
        req.dictated = true;
        let first = super::spawn_prompt(&req, &[]).unwrap();
        assert!(cctui_proto::injected::is_dictated_prompt(&first), "{first}");
        assert!(first.ends_with("\n\nfix the login bug"), "{first}");
        req.prompt = Some("  ".to_owned());
        assert_eq!(super::spawn_prompt(&req, &[]), None);
    }

    #[test]
    fn a_tagged_draft_prompt_is_stored_masked_and_unsealed_real() {
        let key = vec![7u8; 32];
        let meta = draft_metadata(request("m", TAGGED), || key.clone()).unwrap();
        assert!(!meta.to_string().contains("hunter2-xyz"), "{meta}");
        assert_eq!(meta["draft"]["prompt"], MASKED);

        let mut req: SpawnRequest = serde_json::from_value(meta["draft"].clone()).unwrap();
        unseal_draft_prompt(&mut req, &meta, || key.clone()).unwrap();
        assert_eq!(req.prompt.as_deref(), Some(TAGGED));
    }

    #[test]
    fn a_plain_draft_prompt_needs_no_key_and_seals_nothing() {
        let meta = draft_metadata(request("m", "plain"), || panic!("no span, no key")).unwrap();
        assert_eq!(meta["draft"]["prompt"], "plain");
        assert!(meta.get(DRAFT_SEALED_PROMPT).is_none());
    }

    #[test]
    fn a_prompt_sealed_in_the_stored_format_still_opens() {
        let key = vec![7u8; 32];
        let mut meta = serde_json::json!({ "draft": request("m", MASKED) });
        meta[DRAFT_SEALED_PROMPT] = serde_json::Value::String(crate::crypto::encrypt(TAGGED, &key));
        let mut req: SpawnRequest = serde_json::from_value(meta["draft"].clone()).unwrap();
        unseal_draft_prompt(&mut req, &meta, || key.clone()).unwrap();
        assert_eq!(req.prompt.as_deref(), Some(TAGGED));
    }

    #[test]
    fn launch_and_scheduled_sends_share_one_sealed_format() {
        let key = vec![7u8; 32];
        let (masked, sealed) = seal_redacted(TAGGED, || key.clone()).unwrap();
        assert_eq!(masked, MASKED);
        let sealed = sealed.expect("a span is sealed");
        assert_eq!(crate::crypto::decrypt(&sealed, &key).as_deref(), Some(TAGGED));

        let (queued, queued_sealed) =
            crate::scheduled_messages::seal(TAGGED, || key.clone()).unwrap();
        assert_eq!(queued, masked);
        let queued_sealed = queued_sealed.expect("a span is sealed");
        assert_eq!(crate::crypto::decrypt(&queued_sealed, &key).as_deref(), Some(TAGGED));

        assert_eq!(seal_redacted(TAGGED, Vec::new), Err(NeedsVaultKey));
        assert_eq!(
            crate::scheduled_messages::seal(TAGGED, Vec::new),
            Err(crate::scheduled_messages::SEAL_NEEDS_KEY)
        );
    }

    #[test]
    fn a_tagged_draft_without_a_vault_key_is_refused() {
        let err = draft_metadata(request("m", TAGGED), Vec::new).unwrap_err();
        assert_eq!(err.status(), axum::http::StatusCode::BAD_REQUEST);
    }
}
