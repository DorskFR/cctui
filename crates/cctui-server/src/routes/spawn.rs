//! `POST /api/v1/sessions/spawn`.
//!
//! Pushes an `AdapterCommand::Spawn` to the targeted daemon over the
//! existing WS command channel. The daemon's adapter resolves the spawn
//! against its underlying agent (claude-code dispatches via the
//! `claude daemon` control socket; codex parity follows in).
//!
//! Failure modes:
//!   * Daemon offline → 503 with hint.
//!   * Machine not owned by the requesting user → 403.
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

use cctui_proto::adapter::{AdapterCommand, AdapterId, BootstrapUploads, SessionSpec};
use cctui_proto::api::{ApiError, LaunchRequest, SpawnRequest, SpawnResponse};
use cctui_proto::ws::DaemonFrameDown;
use uuid::Uuid;

use crate::auth::AuthContext;
use crate::authz::{Shareable, shareable_owner};
use crate::error::{AppError, DB_ERROR};
use crate::registry::MachineCommand;
use crate::state::AppState;
use crate::store::sessions::SessionRowStatus;
use crate::uploads::parse_upload_multipart;

pub fn bad_request(msg: impl Into<String>) -> (StatusCode, Json<ApiError>) {
    (StatusCode::BAD_REQUEST, Json(ApiError { error: msg.into() }))
}

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
    let parsed = parse_upload_multipart(multipart)
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
    // `POST /sessions/{id}/launch`.
    if req.save_draft {
        return save_draft(&state, &ctx, &req).await;
    }

    dispatch_spawn(&state, &ctx, req, uploads, parsed.raw)
        .await
        .map_err(|(code, Json(e))| AppError::new(code, e.error))
}

/// Dispatch a spawn to the targeted daemon. Shared by the immediate spawn path
/// and the draft-launch path so account env is minted + the command
/// dispatched identically. Validates env keys + machine ownership, mints any
/// account gateway env, and pushes `AdapterCommand::Spawn` over the WS.
pub async fn dispatch_spawn(
    state: &AppState,
    ctx: &AuthContext,
    req: SpawnRequest,
    uploads: Vec<cctui_proto::adapter::BootstrapFile>,
    raw_uploads: Vec<crate::uploads::RawUpload>,
) -> Result<(StatusCode, Json<SpawnResponse>), (StatusCode, Json<ApiError>)> {
    let target = validate_spawn(state, ctx, &req).await?;
    let bound = resolve_spawn_account(state, &target, &req).await?;
    execute_spawn(state, target, bound, &req, uploads, raw_uploads).await
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
    /// can be bound to the *real* session id the worker registers as — rather
    /// than the `command_id`, which the worker never knows and so never
    /// reconciles (leaving `account_name` perpetually null + the key icon
    /// dead). codex mints its own thread id and ignores the pre-minted id, so
    /// its tokens still fall back to `command_id` keying.
    pre_session_id: Option<Uuid>,
    /// The id the gateway session token is bound to: the pre-minted real
    /// session id for claude, else the `command_id`.
    token_session_id: String,
}

async fn validate_spawn(
    state: &AppState,
    ctx: &AuthContext,
    req: &SpawnRequest,
) -> Result<SpawnTarget, (StatusCode, Json<ApiError>)> {
    // Validate env keys: shell-style `^[A-Z_][A-Z0-9_]*$`.
    for key in req.env.keys() {
        let ok = !key.is_empty()
            && key.bytes().next().is_some_and(|b| b == b'_' || b.is_ascii_uppercase())
            && key.bytes().all(|b| b == b'_' || b.is_ascii_uppercase() || b.is_ascii_digit());
        if !ok {
            return Err(bad_request(format!("invalid env key {key:?} (want ^[A-Z_][A-Z0-9_]*$)")));
        }
    }

    let (machine_uuid, owner) = resolve_owned_machine(state, ctx, &req.machine_id).await?;

    let adapter_id = req.adapter_id.clone().unwrap_or_else(|| "claude-code".to_owned());
    let command_id = Uuid::new_v4();
    let is_claude = adapter_id == "claude-code";
    let pre_session_id = is_claude.then(Uuid::new_v4);
    let token_session_id = pre_session_id.unwrap_or(command_id).to_string();
    if req.auto_archive {
        crate::auto_archive::remember_intent(state, &token_session_id).await;
    }
    crate::spawn_labels::remember_intent(&state.pool, &token_session_id, &req.label_ids).await;
    crate::followup::remember_intent(&state.pool, &token_session_id, req).await;
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
        token_session_id,
    })
}

/// The account binding outcome and the session settings it may have rewritten.
struct BoundAccount {
    /// Request env plus any minted gateway env.
    env: std::collections::BTreeMap<String, String>,
    /// The session's model after per-account alias remapping (e.g. `opus` →
    /// `claude-opus-4-8[1m]`).
    model: Option<String>,
    effort: Option<String>,
    permission_mode: Option<cctui_proto::adapter::PermissionMode>,
    account_choice: Option<String>,
}

/// OAuth account selection: if the caller picked a named account, mint a
/// session-scoped gateway token bound to it and inject the gateway base-url +
/// token into the worker env. Raw OAuth tokens never leave the server. With
/// no account named, see [`default_account_name`].
async fn resolve_spawn_account(
    state: &AppState,
    target: &SpawnTarget,
    req: &SpawnRequest,
) -> Result<BoundAccount, (StatusCode, Json<ApiError>)> {
    let SpawnTarget { uid, adapter_id, .. } = target;
    let uid = *uid;
    let mut env = req.env.clone();
    let mut model = req.model.clone().filter(|m| !m.trim().is_empty());
    // Session-provided effort/permission_mode pass through as-is; an unset
    // field falls back to the adapter's/claude's own default.
    let effort = req.effort.clone().filter(|e| !e.trim().is_empty());
    let permission_mode = req.permission_mode;
    let decision = decide_account(
        req.account.as_deref(),
        req.no_account,
        req.auto_account,
        req.pool.as_deref(),
    );
    let auto_bound = matches!(
        decision,
        AccountDecision::ResolveDefault | AccountDecision::Auto | AccountDecision::Pool(_)
    );
    let picked_for_you = matches!(decision, AccountDecision::Auto | AccountDecision::Pool(_));
    // The pool the session may later be moved inside, stamped on its token
    // once the account is minted. `None` for every other decision: a session
    // that named no pool is never moved.
    let mut bound_pool: Option<Uuid> = None;
    let family_for_binding = crate::routes::gateway::Family::from_adapter(adapter_id);
    let account_choice = match decision {
        // A name is an account name first; it only elects a pool when no
        // account of the user's answers to it.
        AccountDecision::Named(a) => {
            let bound = crate::account_resolve::resolve_account_or_pool(
                state,
                uid,
                family_for_binding,
                model.as_deref(),
                &a,
            )
            .await
            .map_err(resolve_err)?;
            bound_pool = bound.pool_id;
            Some(bound.account)
        }
        AccountDecision::Unbound => None,
        AccountDecision::ResolveDefault => default_account_name(state, uid, adapter_id).await?,
        AccountDecision::Auto => {
            auto_account_name(state, uid, adapter_id, model.as_deref()).await?
        }
        AccountDecision::Pool(name) => {
            let (account, pool_id) = crate::account_resolve::resolve_pool(
                state,
                uid,
                family_for_binding,
                model.as_deref(),
                &name,
            )
            .await
            .map_err(resolve_err)?;
            bound_pool = Some(pool_id);
            Some(account)
        }
    };
    if let Some(account_name) = account_choice.as_deref() {
        let acct_ref = if picked_for_you {
            format!("the auto-selected account {account_name:?}")
        } else if auto_bound {
            format!("your default account {account_name:?}")
        } else {
            format!("account {account_name:?}")
        };
        mint_account_env(state, target, account_name, &acct_ref, bound_pool, &mut env, &mut model)
            .await?;
    }
    Ok(BoundAccount { env, model, effort, permission_mode, account_choice })
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
    if crate::routes::gateway::Family::from_adapter(adapter_id)
        == crate::routes::gateway::Family::Openai
    {
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

/// Resolve `model` through the account's alias map and mint the gateway env
/// for the session. Resolution is by (account identity, harness family): the
/// adapter names the family, and the identity carries at most one provider
/// row per family.
async fn mint_account_env(
    state: &AppState,
    target: &SpawnTarget,
    account_name: &str,
    acct_ref: &str,
    bound_pool: Option<Uuid>,
    env: &mut std::collections::BTreeMap<String, String>,
    model: &mut Option<String>,
) -> Result<(), (StatusCode, Json<ApiError>)> {
    let SpawnTarget { uid, adapter_id, token_session_id, .. } = target;
    let uid = *uid;
    let family = crate::routes::gateway::Family::from_adapter(adapter_id);
    // The fireworks family resolves even an ABSENT model: its catalog is the
    // only source of model ids, and its harness has no default to fall back on.
    if model.is_some() || family == crate::routes::gateway::Family::Fireworks {
        let requested = model.as_deref().unwrap_or_default();
        let resolved = crate::routes::gateway::resolve_account_model(
            state,
            uid,
            account_name,
            family,
            requested,
        )
        .await;
        *model = (!resolved.is_empty()).then_some(resolved);
    }
    match crate::routes::gateway::mint_session_env_all_families(
        state,
        uid,
        account_name,
        family,
        token_session_id,
    )
    .await
    {
        Ok(gateway_env) => {
            env.extend(gateway_env);
            if let Some(pool_id) = bound_pool {
                crate::account_resolve::stamp_pool(state, token_session_id, pool_id).await;
            }
            Ok(())
        }
        Err(crate::routes::gateway::MintSessionEnvError::NoAccount) => Err((
            StatusCode::NOT_FOUND,
            Json(ApiError {
                error: format!("{acct_ref} does not exist — connect it on the accounts page"),
            }),
        )),
        Err(crate::routes::gateway::MintSessionEnvError::NoProviderForFamily(f)) => Err((
            StatusCode::NOT_FOUND,
            Json(ApiError {
                error: format!(
                    "{acct_ref} has no {} provider (required by adapter \
                     {adapter_id:?}) — connect one on the accounts page",
                    f.label()
                ),
            }),
        )),
        Err(crate::routes::gateway::MintSessionEnvError::Db(e)) => {
            tracing::error!("mint_session_env failed: {e}");
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError { error: "could not provision account session".into() }),
            ))
        }
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
        token_session_id,
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
        prompt: req.prompt.clone(),
        name: req.name.clone(),
        permission_mode,
        effort,
        model,
        service_tier,
        env,
        bootstrap,
        parent_local_id: None,
    };
    persist_spawn_capability(state, req, permission_mode, &token_session_id).await;
    // `command_id` travels with the command and comes back in an
    // `AdapterEvent::CommandResult` → `ServerEvent::CommandResult`, letting the
    // client surface success/failure instead of silently polling.
    let frame = DaemonFrameDown::Command {
        adapter_id: adapter_id.clone(),
        command: Box::new(AdapterCommand::Spawn {
            spec,
            command_id: Some(command_id),
            session_id: pre_session_id,
        }),
    };

    crate::state::track_command(
        &state.pending_commands,
        command_id,
        None,
        Some(crate::state::FailedSpawnRow {
            session_id: token_session_id.clone(),
            machine_id: machine_uuid,
            user_id: owner,
            adapter_id: adapter_id.clone(),
            working_dir: req.working_dir.clone(),
            name: req.name.clone().filter(|n| !n.trim().is_empty()),
            model: spec_model,
            effort: spec_effort,
        }),
    );
    if let Err(err) = state.bus.command_daemon(machine_uuid, frame).await {
        state.pending_commands.remove(&command_id);
        if let Err(e) =
            crate::routes::attachments::delete_attachments(&state.pool, &recorded_ids).await
        {
            tracing::warn!(session = %token_session_id, "undoing bootstrap attachments: {e}");
        }
        return Err(match err {
            crate::bus::BusError::NoDaemon(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ApiError {
                    error: "daemon for that machine is offline — start `cctui-daemon` first".into(),
                }),
            ),
            _ => (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ApiError { error: "daemon disconnected mid-dispatch".into() }),
            ),
        });
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

/// Keyed by the id the worker will register as, and stored before dispatch so
/// the capability resolves the moment the worker asks.
async fn persist_spawn_capability(
    state: &AppState,
    req: &SpawnRequest,
    permission_mode: Option<cctui_proto::adapter::PermissionMode>,
    token_session_id: &str,
) {
    let mut cap = req
        .spawn_capability
        .clone()
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| state.config.spawn_default_capability());
    let launched = permission_mode.unwrap_or(cctui_proto::adapter::PermissionMode::Ask);
    cap.max_permission_mode = Some(
        cap.max_permission_mode
            .map_or(launched, |c| cctui_proto::adapter::PermissionMode::stricter(c, launched)),
    );
    if let Err(e) =
        crate::store::spawn_capabilities::upsert(&state.pool, token_session_id, &cap).await
    {
        tracing::error!(
            session = %token_session_id,
            error = %e,
            "spawn-capability persist failed — CctuiAgent will be lost on server restart"
        );
    }
    state.spawn_capabilities.insert(token_session_id.to_owned(), cap);
}

/// Account to bind when a spawn names none: the only one (owned or shared) in
/// the adapter's family, `400` listing them if several, unbound if none. An
/// unbound desktop worker would run on the machine owner's ambient login, so
/// the server never guesses between accounts.
async fn default_account_name(
    state: &AppState,
    user_id: Uuid,
    adapter_id: &str,
) -> Result<Option<String>, (StatusCode, Json<ApiError>)> {
    let family = crate::routes::gateway::Family::from_adapter(adapter_id);
    let names: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT a.name \
         FROM account_providers ap JOIN accounts a ON a.id = ap.account_id \
         WHERE ap.family = $2 \
           AND (a.user_id = $1 OR EXISTS ( \
               SELECT 1 FROM resource_shares s \
                WHERE s.resource_type = 'account' AND s.resource_id = a.id \
                  AND s.grantee_id = $1 AND s.revoked_at IS NULL)) \
         ORDER BY a.name",
    )
    .bind(user_id)
    .bind(family.label())
    .fetch_all(&state.pool)
    .await
    .map_err(|e| AppError::from(e).into_parts())?;
    resolve_default_account(&names, user_id, adapter_id)
}

/// Pick the account an `auto_account` spawn binds: the one with the most
/// allocation left for the model it will run.
///
/// Reads every candidate the caller may use in this adapter's provider family,
/// resolves each through any live redirect chain (a rule moves new sessions to
/// another account at mint time, so ranking the origin would score an account
/// that will not serve), then reads each one's usage and hands the lot to
/// [`crate::account_pick::pick_account`].
///
/// Usage comes from the same slow-refresh per-provider cache the accounts page
/// and the gateway's soft-limit check use, so a warm cache costs nothing and a
/// cold one costs a single tokenless call per account, made concurrently. An
/// account whose usage cannot be read is ranked but never treated as
/// exhausted — see the fail-open note on `pick_account`.
async fn auto_account_name(
    state: &AppState,
    user_id: Uuid,
    adapter_id: &str,
    model: Option<&str>,
) -> Result<Option<String>, (StatusCode, Json<ApiError>)> {
    let family = crate::routes::gateway::Family::from_adapter(adapter_id);
    let rows: Vec<(Uuid, String, Uuid, Option<serde_json::Value>)> = sqlx::query_as(
        "SELECT a.id, a.name, ap.id, ap.soft_limits_json \
         FROM account_providers ap JOIN accounts a ON a.id = ap.account_id \
         WHERE ap.family = $2 \
           AND (a.user_id = $1 OR EXISTS ( \
               SELECT 1 FROM resource_shares s \
                WHERE s.resource_type = 'account' AND s.resource_id = a.id \
                  AND s.grantee_id = $1 AND s.revoked_at IS NULL)) \
         ORDER BY a.name",
    )
    .bind(user_id)
    .bind(family.label())
    .fetch_all(&state.pool)
    .await
    .map_err(|e| AppError::from(e).into_parts())?;
    if rows.is_empty() {
        // No accounts configured: unbound spawn, exactly as an unset `account`.
        return Ok(None);
    }

    // Redirect rules are best-effort: failing to read them must not fail the
    // spawn, it only means we rank the origin accounts.
    let rules = crate::store::account_redirects::live_for_launch(&state.pool, user_id)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!("auto account: reading redirects failed, ranking origins: {e}");
            Vec::new()
        });
    // account id → the provider row whose usage represents it.
    let providers: std::collections::HashMap<Uuid, Uuid> =
        rows.iter().map(|(account_id, _, provider_id, _)| (*account_id, *provider_id)).collect();

    // Follow the chain to whoever will actually serve; fall back to the
    // account's own credential when the target is not one we can read. Usage
    // and the sessions already in flight are both measured on that credential.
    let effective: Vec<Uuid> = rows
        .iter()
        .map(|(account_id, _, provider_id, _)| {
            crate::store::account_redirects::follow_account_chain(
                &rules,
                *account_id,
                family.label(),
            )
            .and_then(|to| providers.get(&to).copied())
            .unwrap_or(*provider_id)
        })
        .collect();
    let usages = futures_util::future::join_all(effective.iter().map(|provider| async move {
        crate::routes::gateway::usage_for_soft_limit(state, *provider).await
    }))
    .await;
    let in_flight = crate::account_resolve::in_flight_by_provider(state, &effective).await;

    let candidates: Vec<crate::account_pick::Candidate> = rows
        .iter()
        .zip(usages)
        .zip(&effective)
        .map(|(((_, name, _, soft_limits_json), usage), provider)| crate::account_pick::Candidate {
            name: name.clone(),
            windows: usage
                .as_ref()
                .map(crate::soft_limit::normalize_usage_windows)
                .unwrap_or_default(),
            limits: crate::soft_limit::SoftLimits::from_json(soft_limits_json.as_ref()),
            usage_known: usage.is_some(),
            in_flight: in_flight.get(provider).copied().unwrap_or(0),
        })
        .collect();

    match crate::account_pick::pick_account(&candidates, model, chrono::Utc::now()) {
        crate::account_pick::Pick::Chosen { name, headroom_pct, score, resets_at } => {
            let in_flight = candidates.iter().find(|c| c.name == name).map_or(0, |c| c.in_flight);
            tracing::info!(
                %user_id, account = %name, %adapter_id, headroom_pct, score,
                resets_at = resets_at.map(|t| t.to_rfc3339()), in_flight,
                "auto account: bound the account with the most allocation left"
            );
            Ok(Some(name))
        }
        crate::account_pick::Pick::Exhausted(blocked) => {
            let detail = blocked
                .iter()
                .map(|b| format!("{}: {}", b.name, b.reason))
                .collect::<Vec<_>>()
                .join("; ");
            Err(bad_request(format!("no account has allocation left for this session ({detail})")))
        }
        crate::account_pick::Pick::None => Ok(None),
    }
}

/// Map a shared resolver failure onto the spawn error surface.
fn resolve_err(e: crate::account_resolve::ResolveError) -> (StatusCode, Json<ApiError>) {
    match e {
        crate::account_resolve::ResolveError::Rejected(msg) => bad_request(msg),
        crate::account_resolve::ResolveError::Db => {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: DB_ERROR.into() }))
        }
    }
}

/// The account path a spawn resolves to before any DB default lookup.
#[derive(Debug, PartialEq, Eq)]
enum AccountDecision {
    /// The caller named an account explicitly — always binds it.
    Named(String),
    /// The caller asked for an explicit unbound spawn (`no_account`): skip
    /// `default_account_name`, run on the machine's own ambient login.
    Unbound,
    /// No account named, no unbound request: fall back to the single
    /// matching-family account, if any (auto-bind).
    ResolveDefault,
    /// The caller delegated the choice (`auto_account`): rank every candidate
    /// by remaining allocation and bind the roomiest.
    Auto,
    /// The caller named a pool: choose among its members only, by the pool's
    /// own strategy, and remember the pool on the session.
    Pool(String),
}

/// Pure so the "`no_account` bypasses default resolution" contract is testable
/// without a DB. A named account wins even if `no_account` is set, so
/// a stale flag can never suppress an explicit pick.
fn decide_account(
    account: Option<&str>,
    no_account: bool,
    auto_account: bool,
    pool: Option<&str>,
) -> AccountDecision {
    let pool = pool.map(str::trim).filter(|p| !p.is_empty());
    match account.map(str::trim).filter(|a| !a.is_empty()) {
        Some(a) => AccountDecision::Named(a.to_owned()),
        None if no_account => AccountDecision::Unbound,
        // A pool is a narrower instruction than `auto_account` ("these
        // accounts" vs "any account"), so it wins when both are set rather
        // than silently widening the set the caller asked for.
        None => match pool {
            Some(p) => AccountDecision::Pool(p.to_owned()),
            None if auto_account => AccountDecision::Auto,
            None => AccountDecision::ResolveDefault,
        },
    }
}

/// The 0/1/N decision over the family-filtered candidate names, split
/// from the DB query so it is unit-testable.
fn resolve_default_account(
    names: &[String],
    user_id: Uuid,
    adapter_id: &str,
) -> Result<Option<String>, (StatusCode, Json<ApiError>)> {
    match names {
        [] => Ok(None),
        [one] => {
            tracing::info!(%user_id, account = %one, %adapter_id, "spawn named no account — binding the user's only matching account");
            Ok(Some(one.clone()))
        }
        many => Err(bad_request(format!(
            "no account specified and several are available ({}) — pass `account` to pick one",
            many.join(", ")
        ))),
    }
}

/// Resolve `machine_id` (a UUID) to `(machine_uuid, owner)`, enforcing
/// `admin || caller == owner`.
pub async fn resolve_owned_machine(
    state: &AppState,
    ctx: &AuthContext,
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
    if !(ctx.is_admin() || ctx.user_id == owner) {
        return Err((StatusCode::FORBIDDEN, Json(ApiError { error: "not your machine".into() })));
    }
    Ok((machine_uuid, owner))
}

/// Persist a spawn payload as a `draft` session row. No env is stored
/// (re-entered at launch), no daemon dispatch happens, and the row is excluded
/// from liveness/reaping via its sticky `draft` status. Returns the new draft
/// session id in `command_id` with `status = "draft"`.
async fn save_draft(
    state: &AppState,
    ctx: &AuthContext,
    req: &SpawnRequest,
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

    crate::spawn_labels::sync_draft(&state.pool, &draft_id.to_string(), &req.label_ids).await;
    tracing::info!(machine = %req.machine_id, draft = %draft_id, "draft session saved");
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
    let row: Option<(String, serde_json::Value)> =
        sqlx::query_as("SELECT status, metadata FROM sessions WHERE id = $1")
            .bind(&session_id)
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
    req.env = launch.env;
    req.save_draft = false;
    // Labels put on the draft card after it was saved travel with the launch.
    for id in crate::spawn_labels::draft_label_ids(&state.pool, &session_id).await {
        if !req.label_ids.contains(&id) {
            req.label_ids.push(id);
        }
    }

    let outcome = dispatch_spawn(&state, &ctx, req, Vec::new(), Vec::new())
        .await
        .map_err(|(code, Json(e))| AppError::new(code, e.error))?;

    // Drop the draft only after a successful dispatch; the live session is born
    // from the daemon's registration with its own id.
    if let Err(e) = sqlx::query("DELETE FROM sessions WHERE id = $1 AND status = 'draft'")
        .bind(&session_id)
        .execute(&state.pool)
        .await
    {
        tracing::warn!(%session_id, "draft launched but row delete failed: {e}");
    }
    tracing::info!(draft = %session_id, "draft launched");
    Ok(outcome)
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
    let parsed = parse_upload_multipart(multipart)
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
pub async fn get_machine_commands(
    State(state): State<AppState>,
    Path(machine_id): Path<String>,
) -> Json<Vec<MachineCommand>> {
    let commands = {
        let mut registry = state.registry.write().await;
        registry.take_machine_commands(&machine_id)
    };
    Json(commands)
}

#[cfg(test)]
mod tests {
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
}
