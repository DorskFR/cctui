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
use crate::registry::MachineCommand;
use crate::state::AppState;
use crate::uploads::parse_upload_multipart;

fn bad_request(msg: impl Into<String>) -> (StatusCode, Json<ApiError>) {
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
#[allow(clippy::too_many_lines)]
pub async fn spawn_session(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    multipart: Multipart,
) -> Result<(StatusCode, Json<SpawnResponse>), (StatusCode, Json<ApiError>)> {
    let parsed = parse_upload_multipart(multipart).await?;
    let uploads = parsed.files;
    let req: SpawnRequest = parsed
        .request_json
        .ok_or_else(|| bad_request("missing `request` part"))
        .and_then(|raw| {
            serde_json::from_str(&raw)
                .map_err(|e| bad_request(format!("invalid SpawnRequest JSON: {e}")))
        })?;

    // Draft: stage the spawn payload as a `draft` session row and stop
    // — no env minted, no daemon dispatch, no model turn. Launched later via
    // `POST /sessions/{id}/launch`.
    if req.save_draft {
        return save_draft(&state, &ctx, &req).await;
    }

    dispatch_spawn(&state, &ctx, req, uploads).await
}

/// Dispatch a spawn to the targeted daemon. Shared by the immediate spawn path
/// and the draft-launch path so account env is minted + the command
/// dispatched identically. Validates env keys + machine ownership, mints any
/// account gateway env, and pushes `AdapterCommand::Spawn` over the WS.
#[allow(clippy::too_many_lines)]
async fn dispatch_spawn(
    state: &AppState,
    ctx: &AuthContext,
    req: SpawnRequest,
    uploads: Vec<cctui_proto::adapter::BootstrapFile>,
) -> Result<(StatusCode, Json<SpawnResponse>), (StatusCode, Json<ApiError>)> {
    // Validate env keys: shell-style `^[A-Z_][A-Z0-9_]*$`.
    for key in req.env.keys() {
        let ok = !key.is_empty()
            && key.bytes().next().is_some_and(|b| b == b'_' || b.is_ascii_uppercase())
            && key.bytes().all(|b| b == b'_' || b.is_ascii_uppercase() || b.is_ascii_digit());
        if !ok {
            return Err(bad_request(format!("invalid env key {key:?} (want ^[A-Z_][A-Z0-9_]*$)")));
        }
    }

    let machine_uuid = Uuid::parse_str(&req.machine_id).map_err(|_| {
        (StatusCode::BAD_REQUEST, Json(ApiError { error: "machine_id must be a uuid".into() }))
    })?;
    let row: Option<(Uuid,)> = sqlx::query_as("SELECT user_id FROM machines WHERE id = $1")
        .bind(machine_uuid)
        .fetch_optional(&state.pool)
        .await
        .map_err(|e| {
            tracing::error!("db error: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "database error".into() }))
        })?;
    let Some((owner,)) = row else {
        return Err((StatusCode::NOT_FOUND, Json(ApiError { error: "machine not found".into() })));
    };
    let permitted = ctx.is_admin() || ctx.user_id == owner;
    if !permitted {
        return Err((StatusCode::FORBIDDEN, Json(ApiError { error: "not your machine".into() })));
    }

    // Replica-aware forwarding: if a live peer replica holds this
    // machine's daemon WS, hand the request over before any command/env

    let adapter_id = req.adapter_id.clone().unwrap_or_else(|| "claude-code".to_owned());

    // OAuth account selection: if the caller picked a named account,
    // mint a session-scoped gateway token bound to it and inject the gateway
    // base-url + token into the worker env. Raw OAuth tokens never leave the
    // server.
    //
    // For claude-code we pre-mint the session id here and hand it to
    // the worker as `--session-id` (mirroring the fork path), so the token can
    // be bound to the *real* session id the worker registers as — rather than
    // the command_id, which the worker never knows and so never reconciles
    // (leaving `account_name` perpetually null + the key icon dead). codex
    // mints its own thread id and ignores the pre-minted id, so its tokens
    // still fall back to command_id keying (account_name stays unresolved for
    // codex until a codex-side reconcile lands).
    let command_id = Uuid::new_v4();
    let is_claude = adapter_id == "claude-code";
    let pre_session_id = is_claude.then(Uuid::new_v4);
    // The id the gateway session token is bound to: the pre-minted real session
    // id for claude, else the command_id (legacy behaviour).
    let token_session_id = pre_session_id.unwrap_or(command_id).to_string();
    let mut env = req.env.clone();
    // The session's model before any per-account remapping. When a
    // named account is selected below, its alias map can rewrite this to a
    // concrete model id (e.g. `opus` → `claude-opus-4-8[1m]`).
    let mut model = req.model.clone().filter(|m| !m.trim().is_empty());
    // Session-provided effort/permission_mode pass through as-is. The
    // per-account launch defaults were dropped with the schema split
    // (superseded by per-(machine, cwd) client memory); an unset field
    // falls back to the adapter's/claude's own default.
    let effort = req.effort.clone().filter(|e| !e.trim().is_empty());
    let permission_mode = req.permission_mode;
    // Accounts are user-owned. The admin token has no user identity, so it
    // resolves the account against the target machine's owner —
    // the session runs on that user's machine with that user's account.
    let uid = ctx.owner_filter().unwrap_or(owner);
    // Single source of truth for credentials: an unspecified account
    // no longer silently means "run on whatever ambient login the machine
    // has" — that spawned sessions whose traffic bypassed the gateway (no
    // usage attribution, no soft limits, no langfuse capture) and, on a
    // desktop, billed the machine owner's personal login regardless of intent.
    // With no account named: exactly one matching-family account → bind it;
    // several → 400 (pick explicitly, never guess); none → unbound as before
    // (setups with no accounts configured keep working).
    let decision = decide_account(req.account.as_deref(), req.no_account);
    let auto_bound = matches!(decision, AccountDecision::ResolveDefault);
    let account_choice = match decision {
        AccountDecision::Named(a) => Some(a),
        AccountDecision::Unbound => None,
        AccountDecision::ResolveDefault => default_account_name(state, uid, &adapter_id).await?,
    };
    if let Some(account_name) = account_choice.as_deref() {
        let acct_ref = if auto_bound {
            format!("your default account {account_name:?}")
        } else {
            format!("account {account_name:?}")
        };
        // Resolution is by (account identity, harness family): the
        // adapter names the family, and the identity carries at most one
        // provider row per family. The request's legacy `provider`
        // hint is no longer consulted.
        let family = crate::routes::gateway::Family::from_adapter(&adapter_id);
        // Resolve the model through this account's alias map
        // before it reaches the worker — a no-op when the account has no
        // matching alias.
        // The fireworks family resolves even an ABSENT model: its catalog is the
        // only source of model ids, and its harness has no default to fall back
        // on.
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
            model = (!resolved.is_empty()).then_some(resolved);
        }
        match crate::routes::gateway::mint_session_env(
            state,
            uid,
            account_name,
            family,
            &token_session_id,
        )
        .await
        {
            Ok(gateway_env) => env.extend(gateway_env),
            Err(crate::routes::gateway::MintSessionEnvError::NoAccount) => {
                return Err((
                    StatusCode::NOT_FOUND,
                    Json(ApiError {
                        error: format!(
                            "{acct_ref} does not exist — connect it on the accounts page"
                        ),
                    }),
                ));
            }
            Err(crate::routes::gateway::MintSessionEnvError::NoProviderForFamily(f)) => {
                return Err((
                    StatusCode::NOT_FOUND,
                    Json(ApiError {
                        error: format!(
                            "{acct_ref} has no {} provider (required by adapter \
                             {adapter_id:?}) — connect one on the accounts page",
                            f.label()
                        ),
                    }),
                ));
            }
            Err(crate::routes::gateway::MintSessionEnvError::Db(e)) => {
                tracing::error!("mint_session_env failed: {e}");
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiError { error: "could not provision account session".into() }),
                ));
            }
        }
    }

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
    let spec = SessionSpec {
        adapter_id: AdapterId::new(&adapter_id),
        working_dir: Some(req.working_dir.clone()),
        prompt: req.prompt.clone(),
        name: req.name.clone(),
        permission_mode,
        effort,
        model,
        env,
        bootstrap,
        parent_local_id: None,
    };
    // Keyed by the id the worker will register as, and stored before dispatch so
    // the capability resolves the moment the worker asks.
    if let Some(cap) = req.spawn_capability.clone().filter(|c| !c.is_empty()) {
        if let Err(e) =
            crate::store::spawn_capabilities::upsert(&state.pool, &token_session_id, &cap).await
        {
            tracing::error!(
                session = %token_session_id,
                error = %e,
                "spawn-capability persist failed — CctuiAgent will be lost on server restart"
            );
        }
        state.spawn_capabilities.insert(token_session_id.clone(), cap);
    }
    // `command_id` (minted above) travels with the command and comes back in an
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

    state.bus.command_daemon(machine_uuid, frame).await.map_err(|err| match err {
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
    })?;

    tracing::info!(machine = %req.machine_id, %command_id, %adapter_id, "spawn dispatched");
    Ok((
        StatusCode::ACCEPTED,
        Json(SpawnResponse { command_id, status: "dispatched".into(), account: account_choice }),
    ))
}

/// Resolve `req.machine_id` (a UUID) to the owning user, enforcing
/// `admin || caller == owner`. Returns the machine UUID on success.
/// Pick the account to bind when a spawn names none.
///
/// Sessions used to launch UNBOUND in this case — their traffic skipped the
/// gateway entirely (no usage attribution, no soft limits, no langfuse trace)
/// and, on a desktop daemon, silently consumed the machine owner's ambient
/// `~/.claude` login whatever account the user believed was in play. Credential
/// choice must have one source of truth:
///
///   * exactly one account (owned or shared) in the adapter's provider family →
///     bind it, exactly as if the caller had named it;
///   * several → `400` listing them — the server never guesses between
///     accounts, that's the caller's decision;
///   * none → `Ok(None)`, unbound spawn as before (no-accounts setups keep
///     working; on k8s an unbound worker has no ambient login to leak to).
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
    .map_err(|e| {
        tracing::error!("resolving default account: {e}");
        (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "database error".into() }))
    })?;
    resolve_default_account(&names, user_id, adapter_id)
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
}

/// Pure so the "`no_account` bypasses default resolution" contract is testable
/// without a DB. A named account wins even if `no_account` is set, so
/// a stale flag can never suppress an explicit pick.
fn decide_account(account: Option<&str>, no_account: bool) -> AccountDecision {
    match account.map(str::trim).filter(|a| !a.is_empty()) {
        Some(a) => AccountDecision::Named(a.to_owned()),
        None if no_account => AccountDecision::Unbound,
        None => AccountDecision::ResolveDefault,
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

async fn resolve_owned_machine(
    state: &AppState,
    ctx: &AuthContext,
    machine_id: &str,
) -> Result<Uuid, (StatusCode, Json<ApiError>)> {
    let machine_uuid =
        Uuid::parse_str(machine_id).map_err(|_| bad_request("machine_id must be a uuid"))?;
    let row: Option<(Uuid,)> = sqlx::query_as("SELECT user_id FROM machines WHERE id = $1")
        .bind(machine_uuid)
        .fetch_optional(&state.pool)
        .await
        .map_err(|e| {
            tracing::error!("db error: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "database error".into() }))
        })?;
    let Some((owner,)) = row else {
        return Err((StatusCode::NOT_FOUND, Json(ApiError { error: "machine not found".into() })));
    };
    if !(ctx.is_admin() || ctx.user_id == owner) {
        return Err((StatusCode::FORBIDDEN, Json(ApiError { error: "not your machine".into() })));
    }
    Ok(machine_uuid)
}

/// Persist a spawn payload as a `draft` session row. No env is stored
/// (re-entered at launch), no daemon dispatch happens, and the row is excluded
/// from liveness/reaping via its sticky `draft` status. Returns the new draft
/// session id in `command_id` with `status = "draft"`.
async fn save_draft(
    state: &AppState,
    ctx: &AuthContext,
    req: &SpawnRequest,
) -> Result<(StatusCode, Json<SpawnResponse>), (StatusCode, Json<ApiError>)> {
    let machine_uuid = resolve_owned_machine(state, ctx, &req.machine_id).await?;
    let adapter_id = req.adapter_id.clone().unwrap_or_else(|| "claude-code".to_owned());

    // Store the spawn config (NOT env — secrets never persisted) under
    // `metadata.draft` so Launch/Edit can reconstruct the SpawnRequest.
    let mut payload = req.clone();
    payload.env.clear();
    payload.save_draft = false;
    let draft_json = serde_json::to_value(&payload).map_err(|e| {
        tracing::error!("serializing draft payload: {e}");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError { error: "could not encode draft".into() }),
        )
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
    .await
    .map_err(|e| {
        tracing::error!("db error (save draft): {e}");
        (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "database error".into() }))
    })?;

    tracing::info!(machine = %req.machine_id, draft = %draft_id, "draft session saved");
    Ok((
        StatusCode::CREATED,
        Json(SpawnResponse { command_id: draft_id, status: "draft".into(), account: None }),
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
) -> Result<(StatusCode, Json<SpawnResponse>), (StatusCode, Json<ApiError>)> {
    let row: Option<(String, serde_json::Value)> =
        sqlx::query_as("SELECT status, metadata FROM sessions WHERE id = $1")
            .bind(&session_id)
            .fetch_optional(&state.pool)
            .await
            .map_err(|e| {
                tracing::error!("db error (launch lookup): {e}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiError { error: "database error".into() }),
                )
            })?;
    let Some((status, metadata)) = row else {
        return Err((StatusCode::NOT_FOUND, Json(ApiError { error: "draft not found".into() })));
    };
    if status != "draft" {
        return Err(bad_request("session is not a draft"));
    }
    let mut req: SpawnRequest = metadata
        .get("draft")
        .cloned()
        .ok_or_else(|| bad_request("draft row missing payload"))
        .and_then(|v| {
            serde_json::from_value(v)
                .map_err(|e| bad_request(format!("corrupt draft payload: {e}")))
        })?;
    // Env is entered fresh at launch; account gateway env is minted in dispatch.
    req.env = launch.env;
    req.save_draft = false;

    let outcome = dispatch_spawn(&state, &ctx, req, Vec::new()).await?;

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
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    let res = sqlx::query("DELETE FROM sessions WHERE id = $1 AND status = 'draft'")
        .bind(&session_id)
        .execute(&state.pool)
        .await
        .map_err(|e| {
            tracing::error!("db error (discard draft): {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "database error".into() }))
        })?;
    if res.rows_affected() == 0 {
        return Err((StatusCode::NOT_FOUND, Json(ApiError { error: "draft not found".into() })));
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
) -> Result<Json<cctui_proto::api::StageFilesResponse>, (StatusCode, Json<ApiError>)> {
    let parsed = parse_upload_multipart(multipart).await?;
    if parsed.files.is_empty() {
        return Err(bad_request("no files in upload"));
    }
    let count = parsed.files.len();
    match crate::bus::stage_files(&state, &session_id, parsed.files).await {
        Ok(paths) => {
            tracing::info!(%session_id, count, "staged mid-chat files");
            Ok(Json(cctui_proto::api::StageFilesResponse { paths }))
        }
        Err(crate::bus::BusError::NotFound) => {
            Err((StatusCode::NOT_FOUND, Json(ApiError { error: "session not found".into() })))
        }
        Err(err @ (crate::bus::BusError::NoDaemon(_) | crate::bus::BusError::Closed)) => Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiError {
                error: format!("{err} — the session's machine is offline; try again"),
            }),
        )),
        Err(crate::bus::BusError::Timeout) => Err((
            StatusCode::GATEWAY_TIMEOUT,
            Json(ApiError { error: "timed out staging files on the session's machine".into() }),
        )),
        Err(err @ crate::bus::BusError::Staging(_)) => {
            Err((StatusCode::BAD_GATEWAY, Json(ApiError { error: err.to_string() })))
        }
        Err(err) => {
            tracing::error!(%session_id, %err, "stage_files dispatch error");
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiError { error: "could not stage files".into() }),
            ))
        }
    }
}

/// Legacy poll endpoint — superseded by WS push. Retained so older
/// clients that still poll get an empty list rather than a 404.
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
            decide_account(Some(" acme "), false),
            AccountDecision::Named("acme".to_owned())
        );
        assert_eq!(decide_account(Some("acme"), true), AccountDecision::Named("acme".to_owned()));
    }

    #[test]
    fn no_account_bypasses_default_resolution() {
        assert_eq!(decide_account(None, true), AccountDecision::Unbound);
        assert_eq!(decide_account(Some("   "), true), AccountDecision::Unbound);
    }

    #[test]
    fn unset_account_resolves_default() {
        assert_eq!(decide_account(None, false), AccountDecision::ResolveDefault);
        assert_eq!(decide_account(Some(""), false), AccountDecision::ResolveDefault);
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
