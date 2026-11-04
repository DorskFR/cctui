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

/// The account a parent is bound to and the user it was bound for: a spawn
/// grantee's tree runs on the grantee's accounts even though the machine owner
/// owns the session.
#[derive(Debug, Clone)]
pub struct ParentBinding {
    pub account: String,
    pub account_user: Uuid,
}

/// `None` for an unbound parent, whose children run unbound too.
pub async fn parent_binding(
    state: &impl ChildExecutionContext,
    parent_session: &str,
    owner: Uuid,
) -> Option<ParentBinding> {
    let account = parent_account_name(state, parent_session).await?;
    let account_user =
        crate::gateway::mint::acting_user(state, parent_session, None).await.unwrap_or(owner);
    Some(ParentBinding { account, account_user })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Landing {
    pub account: String,
    /// Set when the parent's account lacked the family and a member of the
    /// parent's pool was elected instead.
    pub pool: Option<Uuid>,
}

fn refuse(family: crate::gateway::Family, why: std::fmt::Arguments<'_>) -> AppError {
    let family = family.label();
    deny(StatusCode::CONFLICT, format!("could not provision the {family} child: {why}"))
}

fn db_error(context: &str, err: &dyn std::fmt::Display) -> AppError {
    tracing::error!("{context}: {err}");
    deny(StatusCode::INTERNAL_SERVER_ERROR, crate::error::DB_ERROR)
}

/// The account a child in `family` lands on: the parent's own when it carries a
/// credential in that family, else a member of the parent's account pool. A
/// child never borrows any other account: outside a pool the spawn is refused.
pub async fn child_landing(
    state: &impl ChildExecutionContext,
    parent_session: &str,
    binding: &ParentBinding,
    family: crate::gateway::Family,
    model: Option<&str>,
) -> Result<Landing, AppError> {
    let rows =
        crate::gateway::mint::account_provider_rows(state, binding.account_user, &binding.account)
            .await
            .map_err(|e| db_error("spawn-child: reading the parent's account", &e))?;
    let Some(rows) = rows else {
        return Err(refuse(
            family,
            format_args!("the parent's account `{}` no longer exists", binding.account),
        ));
    };
    if rows.iter().any(|r| r.family() == family) {
        return Ok(Landing { account: binding.account.clone(), pool: None });
    }
    let (account, label) = (&binding.account, family.label());
    let missing = format!("the parent's account `{account}` has no {label} credential");
    let Some(pool_id) = parent_pool_id(state, parent_session).await else {
        return Err(refuse(
            family,
            format_args!("{missing}, and the parent is not bound to an account pool"),
        ));
    };
    match crate::account_resolve::resolve_pool_by_id(
        state,
        binding.account_user,
        family,
        model,
        pool_id,
    )
    .await
    {
        Ok((account, pool)) => Ok(Landing { account, pool: Some(pool) }),
        Err(crate::account_resolve::ResolveError::Rejected(why)) => {
            Err(refuse(family, format_args!("{missing}, and the parent's pool has none: {why}")))
        }
        Err(crate::account_resolve::ResolveError::Db) => {
            Err(db_error("spawn-child: electing in the parent's pool", &"resolve failed"))
        }
    }
}

/// `model` as `row` serves it, or `None` when the row's catalog cannot serve
/// it. Never substitutes another model: a fireworks catalog miss is a refusal,
/// not its first entry.
#[must_use]
pub fn servable_model(row: &crate::gateway::mint::ProviderRow, model: &str) -> Option<String> {
    let family = row.family();
    let resolved = crate::gateway::mint::resolve_row_model(row, model, false);
    let serves = crate::account_pick::serves_model(
        family,
        row.models.as_ref(),
        row.model_aliases.as_ref(),
        Some(model),
    );
    if family != crate::gateway::Family::Fireworks {
        return serves.then_some(resolved);
    }
    let has_catalog =
        row.models.as_ref().and_then(serde_json::Value::as_array).is_some_and(|a| !a.is_empty());
    if !has_catalog {
        return Some(resolved);
    }
    crate::gateway::mint::resolve_exact_catalog_model(row.models.as_ref(), &resolved)
}

/// The model the child runs on `account`. A requested model the account cannot
/// serve is refused; only an adapter that needs a model and was given none gets
/// the account's default.
async fn child_model(
    state: &impl ChildExecutionContext,
    account_user: Uuid,
    account: &str,
    family: crate::gateway::Family,
    adapter: &str,
    wanted: Option<&str>,
) -> Result<Option<String>, AppError> {
    let Some(wanted) = wanted else {
        if !cctui_proto::adapter::needs_model(adapter) {
            return Ok(None);
        }
        let resolved =
            crate::gateway::mint::resolve_account_model(state, account_user, account, family, "")
                .await;
        return Ok((!resolved.is_empty()).then_some(resolved));
    };
    let rows = crate::gateway::mint::account_provider_rows(state, account_user, account)
        .await
        .map_err(|e| db_error("spawn-child: reading the child's account", &e))?
        .unwrap_or_default();
    let Some(row) = rows.iter().find(|r| r.family() == family) else {
        return Ok(Some(wanted.to_owned()));
    };
    let flipped =
        crate::gateway::mint::flip_model_redirect(state, account_user, row.id, family, wanted)
            .await;
    servable_model(row, &flipped).map(Some).ok_or_else(|| {
        deny(
            StatusCode::BAD_REQUEST,
            format!(
                "the account `{account}` cannot serve model {wanted:?} for a {adapter} child: its \
                 {} catalog does not list it",
                family.label()
            ),
        )
    })
}

/// Mint the child's gateway env and resolve its model on the account it lands
/// on (see [`child_landing`]). A child of an unbound parent is unbound too
/// (empty env, model as requested).
pub async fn child_account_env(
    state: &impl ChildExecutionContext,
    parent: &Parent,
    adapter: &str,
    family: crate::gateway::Family,
    requested_model: Option<&str>,
    child_key: &str,
) -> Result<(std::collections::BTreeMap<String, String>, Option<String>), AppError> {
    let wanted = requested_model.map(str::trim).filter(|m| !m.is_empty()).map(str::to_owned);
    let Some(binding) = parent_binding(state, &parent.session_id, parent.user_id).await else {
        return Ok((std::collections::BTreeMap::new(), wanted));
    };
    let landing =
        child_landing(state, &parent.session_id, &binding, family, wanted.as_deref()).await?;
    let model = child_model(
        state,
        binding.account_user,
        &landing.account,
        family,
        adapter,
        wanted.as_deref(),
    )
    .await?;
    let env = crate::gateway::mint::mint_session_env(
        state,
        binding.account_user,
        &landing.account,
        family,
        child_key,
    )
    .await
    .map_err(|e| match e {
        crate::gateway::mint::MintSessionEnvError::NoAccount => {
            refuse(family, format_args!("the account `{}` no longer exists", landing.account))
        }
        crate::gateway::mint::MintSessionEnvError::NoProviderForFamily(f) => refuse(
            family,
            format_args!("the account `{}` has no {} credential", landing.account, f.label()),
        ),
        crate::gateway::mint::MintSessionEnvError::Db(err) => {
            db_error("spawn-child: minting the child's gateway env", &err)
        }
    })?;
    if let Some(pool_id) = landing.pool {
        crate::account_resolve::stamp_pool(state, child_key, pool_id).await;
    }
    tracing::info!(
        parent = %parent.session_id, child = %child_key, family = family.label(),
        parent_account = %binding.account, account = %landing.account, pool = ?landing.pool,
        "spawn-child: child bound"
    );
    Ok((env, model))
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

/// A child never self-selects Fast: it inherits the bound account's default,
/// else the standard tier.
async fn child_service_tier(
    state: &impl ChildExecutionContext,
    family: crate::gateway::Family,
    child_key: &str,
) -> Option<String> {
    if family != crate::gateway::Family::Openai {
        return None;
    }
    let account_settings = crate::gateway::mint::resolve_session_settings(state, child_key).await;
    Some(crate::settings_catalog::codex::resolve_service_tier(None, account_settings.as_ref()))
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
    let hook_policy = crate::hook_policy::for_child(
        &state.child_state().pool,
        &parent.session_id,
        req.hook_rules.clone(),
    )
    .await?;

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

    let service_tier = child_service_tier(state, family, &child_key).await;
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
        hook_policy: Some(hook_policy),
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

#[cfg(test)]
mod tests {
    use super::servable_model;
    use crate::gateway::mint::ProviderRow;
    use serde_json::json;

    fn row(
        provider: &str,
        models: Option<serde_json::Value>,
        aliases: Option<serde_json::Value>,
    ) -> ProviderRow {
        ProviderRow {
            id: uuid::Uuid::nil(),
            provider: provider.to_owned(),
            model_aliases: aliases,
            models,
        }
    }

    fn kimi_catalog() -> serde_json::Value {
        json!([
            { "model": "accounts/fireworks/models/kimi-k3", "label": "kimi-k3" },
            { "model": "accounts/fireworks/models/glm-5", "label": "glm-5" },
        ])
    }

    #[test]
    fn a_fireworks_catalog_miss_is_refused_not_swapped_for_its_first_entry() {
        let fireworks = row("fireworks", Some(kimi_catalog()), None);
        assert_eq!(servable_model(&fireworks, "gpt-5.5"), None);
    }

    #[test]
    fn a_fireworks_model_resolves_by_label_id_or_alias() {
        let fireworks = row("fireworks", Some(kimi_catalog()), Some(json!({ "fast": "glm-5" })));
        assert_eq!(
            servable_model(&fireworks, "kimi-k3").as_deref(),
            Some("accounts/fireworks/models/kimi-k3")
        );
        assert_eq!(
            servable_model(&fireworks, "accounts/fireworks/models/glm-5").as_deref(),
            Some("accounts/fireworks/models/glm-5")
        );
        assert_eq!(
            servable_model(&fireworks, "fast").as_deref(),
            Some("accounts/fireworks/models/glm-5")
        );
    }

    #[test]
    fn a_row_without_a_catalog_serves_the_requested_model() {
        assert_eq!(servable_model(&row("fireworks", None, None), "x").as_deref(), Some("x"));
        assert_eq!(
            servable_model(&row("openai", None, None), "gpt-5.5").as_deref(),
            Some("gpt-5.5")
        );
    }

    #[test]
    fn a_closed_anthropic_catalog_refuses_what_it_does_not_list() {
        let local = row("anthropic", Some(json!([{ "model": "qwen-coder" }])), None);
        assert_eq!(servable_model(&local, "claude-opus-5-5"), None);
        assert_eq!(servable_model(&local, "qwen-coder").as_deref(), Some("qwen-coder"));
    }

    #[test]
    fn an_anthropic_alias_is_applied_to_the_served_model() {
        let aliased = row("anthropic", None, Some(json!({ "opus": "claude-opus-5-5" })));
        assert_eq!(servable_model(&aliased, "opus").as_deref(), Some("claude-opus-5-5"));
    }
}
