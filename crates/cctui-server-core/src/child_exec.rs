//! Child launch assembly shared by execution and workspace hosts.
use crate::child_policy::{ChildCount, Usage, release_child};
use crate::error::AppError;
use crate::machine_spawn::MachineAccountContext;
use crate::state::CoreState;
use axum::http::StatusCode;
use cctui_proto::adapter::{AdapterId, PermissionMode, SessionSpec};
use cctui_proto::api::{SpawnCapability, SpawnChildRequest, SpawnChildResponse};
use uuid::Uuid;
pub const AGENT_PROFILE_ENV: &str = "CCTUI_OPENCODE_AGENT";
fn deny(code: StatusCode, msg: impl Into<String>) -> AppError {
    AppError::new(code, msg)
}

pub trait ChildExecutionContext: MachineAccountContext + ChildCount {
    fn child_state(&self) -> &CoreState;
    fn profiles_available(&self) -> bool {
        false
    }
    fn remember_child_context(
        &self,
        parent: &Parent,
        key: &str,
        adapter: &str,
        cwd: Option<&str>,
        request: &SpawnChildRequest,
    ) -> impl std::future::Future<Output = ()> + Send;
    fn deliver_child(
        &self,
        parent: &Parent,
        child: Uuid,
        spec: SessionSpec,
    ) -> impl std::future::Future<Output = Result<(), AppError>> + Send;
}
/// The parent session's launch context, everything a child inherits.
#[derive(Debug)]
pub struct Parent {
    pub session_id: String,
    pub machine_uuid: Uuid,
    pub working_dir: Option<String>,
    pub user_id: Uuid,
    pub permission_mode: Option<PermissionMode>,
}

pub async fn load_parent(
    state: &impl ChildExecutionContext,
    session_id: &str,
    caller: Uuid,
) -> Result<Parent, AppError> {
    type ParentRow = (Option<Uuid>, Option<String>, Option<Uuid>, Option<String>);
    let row: Option<ParentRow> = sqlx::query_as(
        "SELECT machine_uuid, working_dir, user_id, permission_mode FROM sessions WHERE id = $1",
    )
    .bind(session_id)
    .fetch_optional(&state.child_state().pool)
    .await?;
    let Some((machine_uuid, working_dir, user_id, mode)) = row else {
        return Err(deny(StatusCode::NOT_FOUND, "calling session not found"));
    };
    let (Some(machine_uuid), Some(user_id)) = (machine_uuid, user_id) else {
        return Err(deny(StatusCode::CONFLICT, "calling session has no machine/owner"));
    };
    if user_id != caller {
        return Err(deny(StatusCode::FORBIDDEN, "session belongs to another user"));
    }
    Ok(Parent {
        session_id: session_id.to_owned(),
        machine_uuid,
        working_dir,
        user_id,
        permission_mode: mode.as_deref().and_then(PermissionMode::from_session_label),
    })
}

/// Who a child is attributed to: the human who spawned its parent, else the
/// parent's owner. The tree acts for whoever started it.
pub async fn child_spawner(pool: &sqlx::PgPool, parent: &Parent) -> Uuid {
    sqlx::query_scalar::<_, Option<Uuid>>("SELECT spawned_by FROM sessions WHERE id = $1")
        .bind(&parent.session_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .flatten()
        .unwrap_or(parent.user_id)
}

/// The account identity the parent is bound to, resolved so the child can mint
/// its own gateway env under the same account for ITS family (a claude parent
/// spawning an opencode child crosses families).
async fn parent_account_name(
    state: &impl ChildExecutionContext,
    session_id: &str,
) -> Option<String> {
    sqlx::query_scalar(
        "SELECT a.name FROM session_tokens st \
         JOIN account_providers ap ON ap.id = st.account_id \
         JOIN accounts a ON a.id = ap.account_id \
         WHERE st.session_id = $1 \
         ORDER BY (st.revoked_at IS NULL) DESC, st.created_at DESC LIMIT 1",
    )
    .bind(session_id)
    .fetch_optional(&state.child_state().pool)
    .await
    .ok()
    .flatten()
}

/// Mint the child's gateway env and resolve its model under the account that
/// serves the child's family.
///
/// That is the parent's own account whenever it carries a credential in that
/// family. It often does not: crossing families (a claude parent spawning a
/// `codex` child) only works when ONE identity holds both providers, and a
/// deployment is free to keep its openai credential on a SEPARATE account. The
/// spawn capability advertises the adapter either way, so refusing here made
/// `CctuiAgent` promise a child it could never provision. When the parent's
/// account cannot serve the family we therefore elect one that can, among the
/// accounts this user owns or has been shared, by the same ranking an
/// `auto_account` spawn uses — most allocation left first.
///
/// A child of an unbound parent is unbound too (empty env, model as requested).
pub async fn child_account_env(
    state: &impl ChildExecutionContext,
    parent: &Parent,
    adapter: &str,
    family: crate::gateway::Family,
    requested_model: Option<&str>,
    child_key: &str,
) -> Result<(std::collections::BTreeMap<String, String>, Option<String>), AppError> {
    let wanted = requested_model.map(str::trim).filter(|m| !m.is_empty()).map(str::to_owned);
    let Some(parent_account) = parent_account_name(state, &parent.session_id).await else {
        return Ok((std::collections::BTreeMap::new(), wanted));
    };
    // The user the parent's account was bound for: a spawn grantee's tree runs
    // on the grantee's accounts even though the machine owner owns the session.
    let account_user = crate::gateway::mint::acting_user(state, &parent.session_id, None)
        .await
        .unwrap_or(parent.user_id);

    let mut first_error = None;
    let mut elected_pool = None;
    let mut election_refusal = None;
    for account in [Some(parent_account.clone()), None] {
        // The second pass runs only when the parent's own account could not
        // serve the family.
        let account = if let Some(a) = account {
            a
        } else {
            let (elected, pool, refusal) =
                elect_family_account(state, parent, account_user, family, wanted.as_deref()).await;
            election_refusal = refusal;
            match elected {
                Some(elected) if elected != parent_account => {
                    tracing::info!(
                        parent = %parent.session_id, child = %child_key,
                        from = %parent_account, to = %elected, family = family.label(),
                        pool = ?pool,
                        "spawn-child: parent's account carries no credential in the child's \
                         family, electing one that does"
                    );
                    elected_pool = pool;
                    elected
                }
                _ => break,
            }
        };

        let mut model = wanted.clone();
        if model.is_some() || cctui_proto::adapter::needs_model(adapter) {
            let resolved = crate::gateway::mint::resolve_account_model(
                state,
                account_user,
                &account,
                family,
                model.as_deref().unwrap_or_default(),
            )
            .await;
            model = (!resolved.is_empty()).then_some(resolved);
        }
        match crate::gateway::mint::mint_session_env(
            state,
            account_user,
            &account,
            family,
            child_key,
        )
        .await
        {
            Ok(env) => {
                if let Some(pool_id) = elected_pool {
                    crate::account_resolve::stamp_pool(state, child_key, pool_id).await;
                }
                return Ok((env, model));
            }
            // Only a missing provider is worth a second account: a vanished
            // account or a DB fault would fail the same way on any of them.
            Err(e @ crate::gateway::mint::MintSessionEnvError::NoProviderForFamily(_)) => {
                first_error.get_or_insert(e);
            }
            Err(e) => {
                first_error.get_or_insert(e);
                break;
            }
        }
    }

    let why = match first_error {
        Some(crate::gateway::mint::MintSessionEnvError::NoAccount) => {
            "the parent's account no longer exists".to_owned()
        }
        // The election's own refusal is the useful one: "every candidate is out
        // of allocation" sends the operator somewhere very different from
        // "there is no such credential".
        Some(crate::gateway::mint::MintSessionEnvError::NoProviderForFamily(f)) => election_refusal
            .map_or_else(
                || {
                    format!(
                        "no account you own carries a {} provider (the parent's does not, \
                         and no other candidate was found)",
                        f.label()
                    )
                },
                |why| format!("no {} account could be elected: {why}", f.label()),
            ),
        Some(crate::gateway::mint::MintSessionEnvError::Db(err)) => {
            tracing::error!(parent = %parent.session_id, "spawn-child mint failed: {err}");
            crate::error::DB_ERROR.to_owned()
        }
        None => crate::error::DB_ERROR.to_owned(),
    };
    Err(deny(
        StatusCode::CONFLICT,
        format!("could not provision a {} child: {why}", family.label()),
    ))
}

/// An account `account_user` may bind in `family`, ranked as an
/// `auto_account` spawn ranks them. A ranking failure (every candidate out of
/// allocation, or none serving the model) is not an error here: it only means
/// there is no fallback, and the caller reports the original mint failure.
async fn elect_family_account(
    state: &impl ChildExecutionContext,
    parent: &Parent,
    account_user: Uuid,
    family: crate::gateway::Family,
    model: Option<&str>,
) -> (Option<String>, Option<Uuid>, Option<String>) {
    // A pool-bound parent never escapes its pool: the member set is the
    // operator's explicit statement of what this tree may spend.
    if let Some(pool_id) = parent_pool_id(state, &parent.session_id).await {
        return match crate::account_resolve::resolve_pool_by_id(
            state,
            account_user,
            family,
            model,
            pool_id,
        )
        .await
        {
            Ok((account, pool)) => (Some(account), Some(pool), None),
            Err(e) => {
                let why = match e {
                    crate::account_resolve::ResolveError::Rejected(reason) => reason,
                    crate::account_resolve::ResolveError::Db => crate::error::DB_ERROR.to_owned(),
                };
                tracing::warn!(
                    parent = %parent.session_id, family = family.label(), %pool_id,
                    "spawn-child: the parent's pool has no account for this family: {why}"
                );
                (None, None, Some(why))
            }
        };
    }

    match crate::machine_spawn::auto_account_name_scoped(state, account_user, family, model, true)
        .await
    {
        Ok(elected) => (elected, None, None),
        Err((_, e)) => {
            let why = e.0.error;
            tracing::warn!(
                parent = %parent.session_id, family = family.label(),
                "spawn-child: no fallback account for this family: {why}"
            );
            (None, None, Some(why))
        }
    }
}

/// The pool the parent's live gateway binding was stamped with, if any.
async fn parent_pool_id(state: &impl ChildExecutionContext, session_id: &str) -> Option<Uuid> {
    let row: Option<(Option<Uuid>,)> = sqlx::query_as(
        "SELECT pool_id FROM session_tokens \
         WHERE session_id = $1 AND revoked_at IS NULL \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(session_id)
    .fetch_optional(&state.child_state().pool)
    .await
    .unwrap_or_else(|e| {
        tracing::warn!(%session_id, error = %e, "reading the parent's pool failed");
        None
    });
    row.and_then(|r| r.0)
}

/// The caller's capability, from the in-memory cache or the durable table it
/// fronts. A DB error denies rather than grants — the check stays fail-closed.
pub async fn capability_for(
    state: &impl ChildExecutionContext,
    session_id: &str,
) -> Option<SpawnCapability> {
    if let Some(cap) = state.child_state().spawn_capabilities.get(session_id) {
        return Some(cap.clone());
    }
    match crate::store::spawn_capabilities::get(&state.child_state().pool, session_id).await {
        Ok(Some(cap)) => {
            state.child_state().spawn_capabilities.insert(session_id.to_owned(), cap.clone());
            Some(cap)
        }
        Ok(None) => None,
        Err(e) => {
            tracing::error!(%session_id, error = %e, "spawn-capability lookup failed — denying");
            None
        }
    }
}

/// Prepare and deliver a child under its parent's existing capability.
pub async fn spawn_child(
    state: &impl ChildExecutionContext,
    caller: Uuid,
    session_id: &str,
    req: &SpawnChildRequest,
) -> Result<SpawnChildResponse, AppError> {
    if req.agent_profile.as_deref().is_some_and(|p| !p.trim().is_empty())
        && !state.profiles_available()
    {
        return Err(deny(
            StatusCode::BAD_REQUEST,
            "workspace agent profiles are unavailable in the automation distribution",
        ));
    }
    let parent = load_parent(state, session_id, caller).await?;

    let cap = capability_for(state, session_id).await;
    let usage = Usage { parent_mode: parent.permission_mode, ..Usage::default() };
    let child_id = Uuid::new_v4();
    let child_key = child_id.to_string();
    // Persisted before the frame: the child pulls its gateway env the moment it
    // launches, and a missing row there would hand it the unclamped default.
    let (authorized, child_cap) = crate::child_policy::reserve_child(
        state,
        &state.child_state().pool,
        session_id,
        cap.as_ref(),
        req,
        usage,
        &child_key,
    )
    .await?;
    let family = match crate::gateway::Family::from_adapter(&authorized.adapter) {
        Ok(f) => f,
        Err(e) => {
            release_child(&state.child_state().pool, &child_key).await;
            return Err(deny(StatusCode::BAD_REQUEST, e.to_string()));
        }
    };
    let (mut env, model) = match child_account_env(
        state,
        &parent,
        &authorized.adapter,
        family,
        req.model.as_deref(),
        &child_key,
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            release_child(&state.child_state().pool, &child_key).await;
            return Err(e);
        }
    };
    if let Some(profile) = req.agent_profile.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        env.insert(AGENT_PROFILE_ENV.to_owned(), profile.to_owned());
    }

    // A child never self-selects Fast: it inherits the bound account's default,
    // else the standard tier.
    let service_tier = if family == crate::gateway::Family::Openai {
        let account_settings =
            crate::gateway::mint::resolve_session_settings(state, &child_key).await;
        Some(crate::settings_catalog::codex::resolve_service_tier(None, account_settings.as_ref()))
    } else {
        None
    };
    let child_cwd =
        req.cwd.clone().filter(|c| !c.trim().is_empty()).or_else(|| parent.working_dir.clone());
    state
        .remember_child_context(&parent, &child_key, &authorized.adapter, child_cwd.as_deref(), req)
        .await;
    let spec = SessionSpec {
        adapter_id: AdapterId::new(&authorized.adapter),
        working_dir: child_cwd,
        prompt: Some(req.prompt.clone()),
        name: req.name.clone().filter(|n| !n.trim().is_empty()),
        permission_mode: Some(authorized.permission_mode),
        effort: None,
        model,
        service_tier,
        env,
        bootstrap: serde_json::Value::Null,
        parent_local_id: Some(parent.session_id.clone()),
    };
    state.child_state().spawn_capabilities.insert(child_key.clone(), child_cap);

    if let Err(err) = state.deliver_child(&parent, child_id, spec).await {
        state.child_state().spawn_capabilities.remove(&child_key);
        release_child(&state.child_state().pool, &child_key).await;
        return Err(err);
    }

    // The child's dollar budget is session-scoped, so it rides the in-memory
    // per-session map the gateway overlays onto the account's soft limits.
    if let Some(budget) = authorized.budget_usd {
        state.child_state().session_usd_budgets.insert(child_key.clone(), budget);
    }
    tracing::info!(
        parent = %session_id,
        child = %child_key,
        adapter = %authorized.adapter,
        budget = ?authorized.budget_usd,
        "CctuiAgent child spawned",
    );
    Ok(SpawnChildResponse { session_id: child_key, budget_usd: authorized.budget_usd })
}

/// Relay a follow-up only to a direct child owned by the parent's owner.
pub async fn message_child(
    state: &impl ChildExecutionContext,
    caller: Uuid,
    session_id: &str,
    req: &cctui_proto::api::MessageChildRequest,
) -> Result<(), AppError> {
    let parent = load_parent(state, session_id, caller).await?;
    if req.prompt.trim().is_empty() {
        return Err(deny(StatusCode::BAD_REQUEST, "prompt is required"));
    }
    let child = req.session_id.trim();
    if child.is_empty() {
        return Err(deny(StatusCode::BAD_REQUEST, "session_id is required"));
    }
    resolve_child_adapter(&state.child_state().pool, caller, &parent, child).await?;
    crate::bus::dispatch_session(
        &state.child_state().pool,
        &state.child_state().bus,
        child,
        cctui_proto::adapter::AdapterCommand::SendMessage {
            local_id: child.to_owned(),
            text: req.prompt.clone(),
        },
    )
    .await
    .map_err(|err| {
        deny(StatusCode::SERVICE_UNAVAILABLE, format!("could not reach the daemon: {err}"))
    })?;
    tracing::info!(parent = %session_id, %child, "CctuiAgent follow-up relayed");
    Ok(())
}

/// Validate direct-child ownership and adapter availability before delivery.
pub async fn resolve_child_adapter(
    pool: &sqlx::PgPool,
    caller: Uuid,
    parent: &Parent,
    child: &str,
) -> Result<String, AppError> {
    let row: Option<(Option<String>, Option<Uuid>, Option<String>)> = sqlx::query_as(
        "SELECT parent_id, machine_uuid, adapter_id FROM sessions WHERE id = $1 AND user_id = $2",
    )
    .bind(child)
    .bind(caller)
    .fetch_optional(pool)
    .await?;
    let Some((child_parent, child_machine, adapter_id)) = row else {
        return Err(deny(StatusCode::NOT_FOUND, "child session not found"));
    };
    if child_parent.as_deref() != Some(parent.session_id.as_str()) {
        return Err(deny(StatusCode::FORBIDDEN, "session is not a child of this session"));
    }
    if child_machine.is_none() {
        return Err(deny(StatusCode::CONFLICT, "child session has no machine"));
    }
    adapter_id
        .filter(|a| !a.is_empty())
        .ok_or_else(|| deny(StatusCode::CONFLICT, "child session has no adapter"))
}

/// A completion becomes a turn in the parent, so it is capped like one.
pub const MAX_COMPLETION_BYTES: usize = 64 * 1024;

#[derive(serde::Deserialize)]
pub struct ChildCompletion {
    pub text: String,
}

/// Deliver a bounded, neutralized completion only to an owned live parent.
pub async fn deliver_completion(
    state: &impl ChildExecutionContext,
    caller: Uuid,
    child_id: &str,
    text: &str,
) -> Result<String, AppError> {
    use crate::peer_session::{Delivery, deliver, envelope};
    let body = text.trim();
    if body.is_empty() {
        return Err(deny(StatusCode::BAD_REQUEST, "text is required"));
    }
    if body.len() > MAX_COMPLETION_BYTES {
        return Err(deny(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("completion is {} bytes; the cap is {MAX_COMPLETION_BYTES}", body.len()),
        ));
    }
    let owned =
        |node: Option<crate::peer_session::SessionNode>| node.filter(|n| n.user_id == Some(caller));
    let child = owned(crate::peer_session::load_node(&state.child_state().pool, child_id).await?)
        .ok_or_else(|| deny(StatusCode::NOT_FOUND, "child session not found"))?;
    let Some(parent_id) = child.parent_id.as_deref() else {
        return Err(deny(StatusCode::CONFLICT, "session has no parent to report to"));
    };
    let parent = owned(crate::peer_session::load_node(&state.child_state().pool, parent_id).await?)
        .ok_or_else(|| deny(StatusCode::NOT_FOUND, "parent session not found"))?;
    if !parent.is_live() {
        return Err(deny(
            StatusCode::CONFLICT,
            format!("parent session {} is {}", parent.id, parent.state()),
        ));
    }
    let turn = envelope(&child, &crate::envelope_guard::neutralize(body));
    match deliver(state.child_state(), &parent.id, parent.state(), turn).await {
        Delivery::Delivered => {}
        outcome => {
            return Err(deny(
                StatusCode::SERVICE_UNAVAILABLE,
                format!("could not deliver to {}: {}", parent.id, outcome.as_str()),
            ));
        }
    }
    tracing::info!(
        child = %child.id,
        parent = %parent.id,
        "CctuiAgent child completion delivered to the parent",
    );
    Ok(parent.id)
}

/// Host-specific archival side effects after common tree authorization.
pub trait ChildArchiveContext: ChildExecutionContext {
    fn archive_child_tree(
        &self,
        owner: Uuid,
        actor: &str,
        target: &str,
    ) -> impl std::future::Future<Output = Result<bool, sqlx::Error>> + Send;
    fn archive_slots_used(
        &self,
        parent: &str,
    ) -> impl std::future::Future<Output = Result<u32, sqlx::Error>> + Send;
}

/// Archive an owned descendant while retaining historical tree spending.
pub async fn archive_descendant(
    state: &impl ChildArchiveContext,
    caller: Uuid,
    session_id: &str,
    target: &str,
) -> Result<cctui_proto::api::ArchiveChildResponse, AppError> {
    if target.is_empty() {
        return Err(deny(StatusCode::BAD_REQUEST, "session_id is required"));
    }
    ensure_descendant(&state.child_state().pool, caller, session_id, target).await?;
    let mut swept: Vec<String> =
        crate::store::sessions::descendants(&state.child_state().pool, target)
            .await?
            .into_iter()
            .map(|c| c.id)
            .collect();
    swept.push(target.to_owned());
    if !state.archive_child_tree(caller, session_id, target).await? {
        return Err(deny(
            StatusCode::CONFLICT,
            format!(
                "session {target} is pinned by the user; ask them to unpin or archive it \
                 themselves"
            ),
        ));
    }
    let archived: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM sessions WHERE id = ANY($1) AND user_id = $2 AND status = 'archived' ORDER BY id",
    )
    .bind(&swept)
    .bind(caller)
    .fetch_all(&state.child_state().pool)
    .await?;
    let max_children = capability_for(state, session_id).await.and_then(|c| c.max_children);
    let children_used = state.archive_slots_used(session_id).await?;
    tracing::info!(
        parent = %session_id,
        child = %target,
        archived = archived.len(),
        "CctuiAgentArchive released a child",
    );
    Ok(cctui_proto::api::ArchiveChildResponse { archived, children_used, max_children })
}

/// Refuse targets outside the caller's owned descendant tree.
pub async fn ensure_descendant(
    pool: &sqlx::PgPool,
    caller: Uuid,
    parent: &str,
    target: &str,
) -> Result<(), AppError> {
    let in_tree =
        crate::store::sessions::descendants(pool, parent).await?.iter().any(|c| c.id == target);
    let owner: Option<Option<Uuid>> =
        sqlx::query_scalar("SELECT user_id FROM sessions WHERE id = $1")
            .bind(target)
            .fetch_optional(pool)
            .await?;
    if in_tree && owner == Some(Some(caller)) {
        Ok(())
    } else {
        Err(deny(StatusCode::NOT_FOUND, "no such session below this one"))
    }
}
