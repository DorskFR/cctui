//! `POST /api/v1/daemon/sessions/{id}/spawn-child` — the server side of the
//! daemon's `CctuiAgent` tool.
//!
//! A session asks its daemon to spawn a subagent; the daemon relays the request
//! here with its machine key. The server is the only place the decision is made:
//! it reads the calling session's [`SpawnCapability`](cctui_proto::api::SpawnCapability) — set by whoever launched
//! that session, never writable by the session itself — and refuses anything the
//! capability does not name. No capability at all ⇒ deny. A child never runs
//! more permissively than its parent, never deeper than the capability's
//! depth, and the budgets granted across one spawn tree never sum past the
//! root's tree ceiling.
//!
//! An authorized child goes down the ordinary spawn path (pre-minted session id,
//! account-bound gateway env, `AdapterCommand::Spawn` over the daemon WS), with
//! `parent_local_id` set so it registers as a real, nested, meterable session.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;

use cctui_proto::adapter::AdapterCommand;
#[cfg(test)]
use cctui_proto::api::SpawnCapability;
use cctui_proto::api::{
    ApiError, ArchiveChildRequest, ArchiveChildResponse, SpawnChildRequest, SpawnChildResponse,
};
use cctui_proto::ws::DaemonFrameDown;
use uuid::Uuid;

#[cfg(test)]
use cctui_server_core::child_exec::child_account_env;
use cctui_server_core::child_exec::{
    ChildExecutionContext, Parent, capability_for, child_spawner, load_parent,
};

#[cfg(test)]
pub use cctui_server_core::child_policy::{
    Authorized, Denied, Usage, authorize, release_child, tree_granted_usd,
};
use cctui_server_core::child_policy::{ChildCount, pending_child_count};

use crate::error::AppError;
use crate::followup::nests_under_parent;
use crate::state::AppState;

fn deny(code: StatusCode, msg: impl Into<String>) -> AppError {
    AppError::new(code, msg)
}

/// Authenticate the caller as a daemon machine key and return its user id.
pub async fn machine_user(
    state: &AppState,
    headers: &axum::http::HeaderMap,
) -> Result<Uuid, (StatusCode, Json<ApiError>)> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| {
            (StatusCode::UNAUTHORIZED, Json(ApiError { error: "machine key required".into() }))
        })?;
    let ctx = state.auth_config.validate(token).await.ok_or_else(|| {
        (StatusCode::UNAUTHORIZED, Json(ApiError { error: "invalid machine key".into() }))
    })?;
    if ctx.machine_id.is_none() {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ApiError { error: "machine token required".into() }),
        ));
    }
    Ok(ctx.user_id)
}

/// Same resolution the webui spawn runs, keyed on the child's own launch key,
/// so a profile's context set means the same thing from the tool as from the
/// browser. The child's spawner is recorded under the same key.
async fn remember_child_context(
    state: &AppState,
    parent: &Parent,
    child_key: &str,
    adapter: &str,
    cwd: Option<&str>,
    profile: Option<&str>,
) {
    use crate::routes::context::{SpawnScope, StoredScope};
    let scope = StoredScope {
        user_id: parent.user_id,
        coords: SpawnScope {
            machine_id: Some(parent.machine_uuid.to_string()),
            working_dir: cwd.map(str::to_owned),
            label_ids: Vec::new(),
            adapter: Some(adapter.to_owned()),
        },
        dispatched: crate::routes::context::stored_scope(&state.pool, &parent.session_id)
            .await
            .is_some_and(|s| s.dispatched),
    };
    let context = crate::routes::context::resolve_for_child(
        &state.pool,
        parent.user_id,
        profile,
        &scope.coords,
    )
    .await;
    crate::routes::context::remember_intent(&state.pool, child_key, &context, &scope).await;
    let spawner = child_spawner(&state.pool, parent).await;
    crate::routes::spawn::remember_spawner(&state.pool, child_key, spawner, None).await;
}

struct WorkspaceChildCount;
impl ChildCount for WorkspaceChildCount {
    async fn live_child_count(
        &self,
        conn: &mut sqlx::PgConnection,
        parent_id: &str,
    ) -> Result<u32, sqlx::Error> {
        Ok(live_child_count(conn, parent_id).await)
    }
}

#[cfg(test)]
async fn reserve_child(
    pool: &sqlx::PgPool,
    parent_id: &str,
    cap: Option<&SpawnCapability>,
    req: &SpawnChildRequest,
    usage: Usage,
    child_key: &str,
) -> Result<(Authorized, SpawnCapability), AppError> {
    cctui_server_core::child_policy::reserve_child(
        &WorkspaceChildCount,
        pool,
        parent_id,
        cap,
        req,
        usage,
        child_key,
    )
    .await
}

/// Children counting against the parent's spawn quota: every child except those
/// that ended in failure or were archived. A child that emitted a terminal
/// `session_ended` whose reason is anything but `Completed` (crashed, killed,
/// adapter error) has freed its slot, so the parent can respawn a replacement.
/// A completed child that is not archived still counts: the parent can reattach
/// to it by `session_id`.
async fn live_child_count(exec: impl sqlx::PgExecutor<'_>, parent_id: &str) -> u32 {
    let n: i64 = sqlx::query_scalar(concat!(
        "SELECT count(*) FROM sessions s \
         WHERE s.parent_id = $1 AND ",
        nests_under_parent!("s"),
        " AND s.status <> 'archived' \
         AND NOT EXISTS ( \
             SELECT 1 FROM stream_events e \
             WHERE e.session_id = s.id AND e.event_type = 'session_ended' \
             AND e.payload->>'reason' <> 'Completed' \
         )",
    ))
    .bind(parent_id)
    .fetch_one(exec)
    .await
    .unwrap_or(0);
    u32::try_from(n).unwrap_or(u32::MAX)
}

impl ChildCount for AppState {
    async fn live_child_count(
        &self,
        conn: &mut sqlx::PgConnection,
        parent: &str,
    ) -> Result<u32, sqlx::Error> {
        WorkspaceChildCount.live_child_count(conn, parent).await
    }
}
impl ChildExecutionContext for AppState {
    fn child_state(&self) -> &cctui_server_core::state::CoreState {
        &self.core
    }
    fn profiles_available(&self) -> bool {
        true
    }
    async fn remember_child_context(
        &self,
        parent: &Parent,
        key: &str,
        adapter: &str,
        cwd: Option<&str>,
        request: &SpawnChildRequest,
    ) {
        remember_child_context(self, parent, key, adapter, cwd, request.agent_profile.as_deref())
            .await;
    }
    async fn deliver_child(
        &self,
        parent: &Parent,
        child: Uuid,
        spec: cctui_proto::adapter::SessionSpec,
    ) -> Result<(), AppError> {
        let frame = DaemonFrameDown::Command {
            adapter_id: spec.adapter_id.as_str().to_owned(),
            command: Box::new(AdapterCommand::Spawn {
                spec,
                command_id: Some(child),
                session_id: Some(child),
            }),
        };
        self.bus
            .command_daemon_for_session(parent.machine_uuid, &parent.session_id, frame)
            .await
            .map_err(|err| {
                deny(StatusCode::SERVICE_UNAVAILABLE, format!("could not reach the daemon: {err}"))
            })
    }
}

/// `POST /api/v1/daemon/sessions/{id}/spawn-child`.
pub async fn spawn_child(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(session_id): Path<String>,
    Json(req): Json<SpawnChildRequest>,
) -> Result<Json<SpawnChildResponse>, AppError> {
    let caller = machine_user(&state, &headers)
        .await
        .map_err(|(code, Json(e))| AppError::new(code, e.error))?;
    cctui_server_core::child_exec::spawn_child(&state, caller, &session_id, &req).await.map(Json)
}

/// `POST /api/v1/daemon/sessions/{id}/message-child` — a follow-up prompt from
/// parent `{id}` into a child it spawned. Fail-closed: the target must be a
/// direct child of the caller on the caller's machine; a session can never
/// message an arbitrary session this way.
pub async fn message_child(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(session_id): Path<String>,
    Json(req): Json<cctui_proto::api::MessageChildRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let caller = machine_user(&state, &headers)
        .await
        .map_err(|(code, Json(e))| AppError::new(code, e.error))?;
    let parent = load_parent(&state, &session_id, caller).await?;
    if req.prompt.trim().is_empty() {
        return Err(deny(StatusCode::BAD_REQUEST, "prompt is required"));
    }
    let child = req.session_id.trim();
    if child.is_empty() {
        return Err(deny(StatusCode::BAD_REQUEST, "session_id is required"));
    }
    resolve_child_adapter(&state.pool, caller, &parent, child).await?;
    crate::bus::dispatch(
        &state,
        child,
        AdapterCommand::SendMessage { local_id: child.to_owned(), text: req.prompt.clone() },
    )
    .await
    .map_err(|err| {
        deny(StatusCode::SERVICE_UNAVAILABLE, format!("could not reach the daemon: {err}"))
    })?;
    tracing::info!(parent = %session_id, %child, "CctuiAgent follow-up relayed");
    Ok(Json(serde_json::json!({})))
}

/// The follow-up target's adapter, only if `child` really is `parent`'s child
/// owned by `caller`. The child's machine is NOT required to be the parent's:
/// delivery goes through [`crate::bus::dispatch`], which resolves the child's
/// own machine and routes across replicas.
async fn resolve_child_adapter(
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
const MAX_COMPLETION_BYTES: usize = 64 * 1024;

#[derive(serde::Deserialize)]
pub struct ChildCompletion {
    text: String,
}

/// `POST /api/v1/daemon/sessions/{id}/child-completion` — child `{id}`'s
/// finished turn, posted by its daemon when no `CctuiAgent` call is following
/// it any more. Delivered into the parent as a turn from the child.
pub async fn child_completion(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(child_id): Path<String>,
    Json(req): Json<ChildCompletion>,
) -> Result<Json<serde_json::Value>, AppError> {
    let caller = machine_user(&state, &headers)
        .await
        .map_err(|(code, Json(e))| AppError::new(code, e.error))?;
    let parent = deliver_completion(&state, caller, &child_id, &req.text).await?;
    Ok(Json(serde_json::json!({ "delivered_to": parent })))
}

/// The parent's id once `text` is in its turn queue. Both sessions must be
/// `caller`'s, and only a live parent is offered the turn.
async fn deliver_completion(
    state: &AppState,
    caller: Uuid,
    child_id: &str,
    text: &str,
) -> Result<String, AppError> {
    use crate::routes::peer::{Delivery, deliver, envelope};
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
        |node: Option<crate::peer_policy::SessionNode>| node.filter(|n| n.user_id == Some(caller));
    let child = owned(crate::peer_policy::load_node(&state.pool, child_id).await?)
        .ok_or_else(|| deny(StatusCode::NOT_FOUND, "child session not found"))?;
    let Some(parent_id) = child.parent_id.as_deref() else {
        return Err(deny(StatusCode::CONFLICT, "session has no parent to report to"));
    };
    let parent = owned(crate::peer_policy::load_node(&state.pool, parent_id).await?)
        .ok_or_else(|| deny(StatusCode::NOT_FOUND, "parent session not found"))?;
    if !parent.is_live() {
        return Err(deny(
            StatusCode::CONFLICT,
            format!("parent session {} is {}", parent.id, parent.state()),
        ));
    }
    let turn = envelope(&child, &crate::envelope_guard::neutralize(body));
    match deliver(state, &parent.id, parent.state(), turn).await {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct ChildSlots {
    pub used: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depth_left: Option<u32>,
}

pub async fn child_slots(state: &AppState, session_id: &str) -> Option<ChildSlots> {
    let cap = capability_for(state, session_id).await.filter(|c| !c.is_empty())?;
    let used = slots_used(&state.pool, session_id).await;
    Some(ChildSlots { used, max: cap.max_children, depth_left: cap.max_depth })
}

/// The same count `reserve_child` gates on.
async fn slots_used(pool: &sqlx::PgPool, session_id: &str) -> u32 {
    let pending = pending_child_count(pool, session_id).await.unwrap_or_else(|e| {
        tracing::warn!(%session_id, error = %e, "pending child count failed");
        0
    });
    live_child_count(pool, session_id).await.saturating_add(pending)
}

/// `POST /api/v1/daemon/sessions/{id}/archive-child` — `{id}` releases a
/// descendant in its own spawn tree. Anything outside the tree is a 404 so ids
/// do not leak, and a pinned target is always refused: no agent overrides a pin.
/// A running target is killed, and its own descendants are archived with it.
pub async fn archive_child(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(session_id): Path<String>,
    Json(req): Json<ArchiveChildRequest>,
) -> Result<Json<ArchiveChildResponse>, AppError> {
    let caller = machine_user(&state, &headers)
        .await
        .map_err(|(code, Json(e))| AppError::new(code, e.error))?;
    let parent = load_parent(&state, &session_id, caller).await?;
    archive_descendant(&state, caller, &parent.session_id, req.session_id.trim()).await.map(Json)
}

async fn archive_descendant(
    state: &AppState,
    caller: Uuid,
    session_id: &str,
    target: &str,
) -> Result<ArchiveChildResponse, AppError> {
    if target.is_empty() {
        return Err(deny(StatusCode::BAD_REQUEST, "session_id is required"));
    }
    ensure_descendant(&state.pool, caller, session_id, target).await?;
    let mut swept: Vec<String> = crate::store::sessions::descendants(&state.pool, target)
        .await?
        .into_iter()
        .map(|c| c.id)
        .collect();
    swept.push(target.to_owned());
    let outcome = crate::routes::sessions::archive_one(
        state,
        target,
        false,
        cctui_proto::adapter::RemoveInitiator::Automatic,
        crate::events::Actor::Agent(session_id.to_owned()),
    )
    .await?;
    if outcome == crate::routes::sessions::ArchiveOutcome::SkippedPinned {
        return Err(deny(
            StatusCode::CONFLICT,
            format!(
                "session {target} is pinned by the user; ask them to unpin or archive it \
                 themselves"
            ),
        ));
    }
    let archived: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM sessions WHERE id = ANY($1) AND status = 'archived' ORDER BY id",
    )
    .bind(&swept)
    .fetch_all(&state.pool)
    .await?;
    let max_children = capability_for(state, session_id).await.and_then(|c| c.max_children);
    let children_used = slots_used(&state.pool, session_id).await;
    tracing::info!(
        parent = %session_id,
        child = %target,
        archived = archived.len(),
        "CctuiAgentArchive released a child",
    );
    Ok(ArchiveChildResponse { archived, children_used, max_children })
}

async fn ensure_descendant(
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
    use super::*;
    use serde_json::json;

    fn cap(
        adapters: &[&str],
        max_budget: Option<f64>,
        max_children: Option<u32>,
    ) -> SpawnCapability {
        SpawnCapability {
            adapters: adapters.iter().map(|s| (*s).to_owned()).collect(),
            max_budget_usd: max_budget,
            max_children,
            ..SpawnCapability::default()
        }
    }

    fn usage(live_children: u32) -> Usage {
        Usage { live_children, ..Usage::default() }
    }

    fn req(adapter: &str, budget: Option<f64>) -> SpawnChildRequest {
        SpawnChildRequest {
            adapter: adapter.to_owned(),
            prompt: "review this".into(),
            budget_usd: budget,
            ..SpawnChildRequest::default()
        }
    }

    /// DB-gated: a claude parent may spawn a `codex` child even when the
    /// openai credential lives on a DIFFERENT account — the deployment shape
    /// that made `CctuiAgent` answer "the parent's account has no openai
    /// provider" for an adapter its own capability advertised.
    #[tokio::test]
    async fn child_crosses_to_a_sibling_account_for_its_family() {
        let Some(url) = crate::routes::gateway::test_db_url(
            "child_crosses_to_a_sibling_account_for_its_family",
        ) else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");

        let uid = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, 'cross-test', $2)")
            .bind(uid)
            .bind(format!("kh-{uid}"))
            .execute(&pool)
            .await
            .expect("seed user");

        // The parent runs on an anthropic-only identity; the openai credential
        // is a separate account, as a pooled deployment keeps it.
        let mut providers = Vec::new();
        for (name, provider) in [("parent-claude", "anthropic"), ("sibling-codex", "openai")] {
            let account = Uuid::new_v4();
            sqlx::query("INSERT INTO accounts (id, user_id, name) VALUES ($1, $2, $3)")
                .bind(account)
                .bind(uid)
                .bind(name)
                .execute(&pool)
                .await
                .expect("seed account");
            let provider_id = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO account_providers                    (id, user_id, account_id, provider, auth_scheme, encrypted_access_token)                  VALUES ($1, $2, $3, $4, 'api_key', 'x')",
            )
            .bind(provider_id)
            .bind(uid)
            .bind(account)
            .bind(provider)
            .execute(&pool)
            .await
            .expect("seed provider");
            providers.push((provider, provider_id));
        }
        let anthropic_provider = providers[0].1;
        let openai_provider = providers[1].1;

        let parent_id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO session_tokens (token_hash, session_id, account_id) VALUES ($1, $2, $3)",
        )
        .bind(format!("th-{parent_id}"))
        .bind(&parent_id)
        .bind(anthropic_provider)
        .execute(&pool)
        .await
        .expect("bind parent to its anthropic account");

        let state = crate::state::AppState::for_test(pool.clone());
        let parent = Parent {
            session_id: parent_id.clone(),
            machine_uuid: Uuid::new_v4(),
            working_dir: None,
            user_id: uid,
            permission_mode: None,
        };

        let child_key = Uuid::new_v4().to_string();
        let minted = child_account_env(
            &state,
            &parent,
            "codex",
            crate::routes::gateway::Family::Openai,
            None,
            &child_key,
        )
        .await;
        assert!(minted.is_ok(), "codex child refused: {:?}", minted.err().map(|e| e.to_string()));
        let (env, _) = minted.expect("env");
        assert!(!env.is_empty(), "the child got no gateway env");

        // It is the SIBLING's credential the child is bound to, not the
        // parent's: binding the anthropic row would 401 on the first turn.
        let bound: Uuid =
            sqlx::query_scalar("SELECT account_id FROM session_tokens WHERE session_id = $1")
                .bind(&child_key)
                .fetch_one(&pool)
                .await
                .expect("child token");
        assert_eq!(bound, openai_provider, "child bound to the wrong provider row");

        // The parent's own family still binds the parent's own account: the
        // fallback must not hijack a child that needs no crossing.
        let same_family_key = Uuid::new_v4().to_string();
        let (env, _) = child_account_env(
            &state,
            &parent,
            "claude-code",
            crate::routes::gateway::Family::Anthropic,
            None,
            &same_family_key,
        )
        .await
        .expect("claude child");
        assert!(!env.is_empty());
        let bound: Uuid =
            sqlx::query_scalar("SELECT account_id FROM session_tokens WHERE session_id = $1")
                .bind(&same_family_key)
                .fetch_one(&pool)
                .await
                .expect("child token");
        assert_eq!(bound, anthropic_provider, "same-family child left its parent's account");

        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
    }

    /// Seeds a user plus one account per `(name, provider)`, returning the
    /// account and provider ids in order.
    #[cfg(test)]
    async fn seed_accounts(
        pool: &sqlx::PgPool,
        uid: Uuid,
        label: &str,
        accounts: &[(&str, &str)],
    ) -> Vec<(Uuid, Uuid)> {
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("{label}-{uid}"))
            .bind(format!("kh-{uid}"))
            .execute(pool)
            .await
            .expect("seed user");
        let mut out = Vec::new();
        for (name, provider) in accounts {
            let account = Uuid::new_v4();
            sqlx::query("INSERT INTO accounts (id, user_id, name) VALUES ($1, $2, $3)")
                .bind(account)
                .bind(uid)
                .bind(format!("{name}-{account}"))
                .execute(pool)
                .await
                .expect("seed account");
            let provider_id = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO account_providers \
                   (id, user_id, account_id, provider, auth_scheme, encrypted_access_token) \
                 VALUES ($1, $2, $3, $4, 'api_key', 'x')",
            )
            .bind(provider_id)
            .bind(uid)
            .bind(account)
            .bind(provider)
            .execute(pool)
            .await
            .expect("seed provider");
            out.push((account, provider_id));
        }
        out
    }

    /// A parent session bound to `uid`'s own claude account, with a colleague's
    /// codex account shared to `uid`. Returns the parent's session id.
    async fn seed_parent_with_shared_colleague_account(
        pool: &sqlx::PgPool,
        uid: Uuid,
        colleague: Uuid,
    ) -> String {
        let mine = seed_accounts(pool, uid, "owner", &[("parent-claude", "anthropic")]).await;
        let theirs =
            seed_accounts(pool, colleague, "colleague", &[("their-codex", "openai")]).await;
        sqlx::query(
            "INSERT INTO resource_shares (resource_type, resource_id, grantee_id) \
             VALUES ('account', $1, $2)",
        )
        .bind(theirs[0].0)
        .bind(uid)
        .execute(pool)
        .await
        .expect("share the colleague's account with the parent's owner");
        let parent_id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO session_tokens (token_hash, session_id, account_id) VALUES ($1, $2, $3)",
        )
        .bind(format!("th-{parent_id}"))
        .bind(&parent_id)
        .bind(mine[0].1)
        .execute(pool)
        .await
        .expect("bind parent");
        parent_id
    }

    /// An openai provider row on an account `uid` owns; returns the provider id.
    async fn seed_own_openai_provider(pool: &sqlx::PgPool, uid: Uuid) -> Uuid {
        let account = Uuid::new_v4();
        sqlx::query("INSERT INTO accounts (id, user_id, name) VALUES ($1, $2, $3)")
            .bind(account)
            .bind(uid)
            .bind(format!("my-codex-{account}"))
            .execute(pool)
            .await
            .expect("seed own openai account");
        let own_openai = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO account_providers \
               (id, user_id, account_id, provider, auth_scheme, encrypted_access_token) \
             VALUES ($1, $2, $3, 'openai', 'api_key', 'x')",
        )
        .bind(own_openai)
        .bind(uid)
        .bind(account)
        .execute(pool)
        .await
        .expect("seed own provider");
        own_openai
    }

    /// DB-gated R3: an election the operator never asked for must not reach a
    /// colleague's account and spend their allocation, even though the parent's
    /// owner may legitimately launch on it when they name it.
    #[tokio::test]
    async fn an_elected_child_never_binds_an_account_only_shared_with_the_owner() {
        let Some(url) = crate::routes::gateway::test_db_url(
            "an_elected_child_never_binds_an_account_only_shared_with_the_owner",
        ) else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");

        let uid = Uuid::new_v4();
        let colleague = Uuid::new_v4();
        let parent_id = seed_parent_with_shared_colleague_account(&pool, uid, colleague).await;

        let state = crate::state::AppState::for_test(pool.clone());
        let parent = Parent {
            session_id: parent_id.clone(),
            machine_uuid: Uuid::new_v4(),
            working_dir: None,
            user_id: uid,
            permission_mode: None,
        };

        let child_key = Uuid::new_v4().to_string();
        let refused = child_account_env(
            &state,
            &parent,
            "codex",
            crate::routes::gateway::Family::Openai,
            None,
            &child_key,
        )
        .await;
        assert!(refused.is_err(), "the child was provisioned off a colleague's account");
        let bound: Option<Uuid> =
            sqlx::query_scalar("SELECT account_id FROM session_tokens WHERE session_id = $1")
                .bind(&child_key)
                .fetch_optional(&pool)
                .await
                .expect("query child token");
        assert_eq!(bound, None, "a token was minted on someone else's credential");

        // The same shape with an openai account the owner OWNS does provision,
        // so the refusal above is about ownership and not about the wiring.
        let own_openai = seed_own_openai_provider(&pool, uid).await;

        let ok_key = Uuid::new_v4().to_string();
        child_account_env(
            &state,
            &parent,
            "codex",
            crate::routes::gateway::Family::Openai,
            None,
            &ok_key,
        )
        .await
        .expect("a child on the owner's own openai account");
        let bound: Uuid =
            sqlx::query_scalar("SELECT account_id FROM session_tokens WHERE session_id = $1")
                .bind(&ok_key)
                .fetch_one(&pool)
                .await
                .expect("child token");
        assert_eq!(bound, own_openai, "the owner's own account should have been elected");

        for u in [uid, colleague] {
            sqlx::query("DELETE FROM users WHERE id = $1").bind(u).execute(&pool).await.ok();
        }
    }

    /// DB-gated R3: a pool is an explicit statement of what the tree may spend,
    /// so a child elected for another family stays inside it and is stamped
    /// with it.
    #[tokio::test]
    async fn an_elected_child_stays_inside_the_parents_pool() {
        let Some(url) =
            crate::routes::gateway::test_db_url("an_elected_child_stays_inside_the_parents_pool")
        else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");

        let uid = Uuid::new_v4();
        let seeded = seed_accounts(
            &pool,
            uid,
            "pooled",
            &[
                ("parent-claude", "anthropic"),
                ("in-pool-codex", "openai"),
                ("outside-codex", "openai"),
            ],
        )
        .await;
        let (parent_account, anthropic_provider) = seeded[0];
        let (in_pool_account, in_pool_provider) = seeded[1];

        let pool_id = Uuid::new_v4();
        sqlx::query("INSERT INTO account_pools (id, user_id, name) VALUES ($1, $2, $3)")
            .bind(pool_id)
            .bind(uid)
            .bind(format!("team-{pool_id}"))
            .execute(&pool)
            .await
            .expect("seed pool");
        for account in [parent_account, in_pool_account] {
            sqlx::query("INSERT INTO account_pool_members (pool_id, account_id) VALUES ($1, $2)")
                .bind(pool_id)
                .bind(account)
                .execute(&pool)
                .await
                .expect("seed pool member");
        }

        let parent_id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO session_tokens (token_hash, session_id, account_id, pool_id) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(format!("th-{parent_id}"))
        .bind(&parent_id)
        .bind(anthropic_provider)
        .bind(pool_id)
        .execute(&pool)
        .await
        .expect("bind parent to its pool");

        let state = crate::state::AppState::for_test(pool.clone());
        let parent = Parent {
            session_id: parent_id.clone(),
            machine_uuid: Uuid::new_v4(),
            working_dir: None,
            user_id: uid,
            permission_mode: None,
        };

        let child_key = Uuid::new_v4().to_string();
        child_account_env(
            &state,
            &parent,
            "codex",
            crate::routes::gateway::Family::Openai,
            None,
            &child_key,
        )
        .await
        .expect("a codex child from the pool");

        let (bound, stamped): (Uuid, Option<Uuid>) =
            sqlx::query_as("SELECT account_id, pool_id FROM session_tokens WHERE session_id = $1")
                .bind(&child_key)
                .fetch_one(&pool)
                .await
                .expect("child token");
        assert_eq!(bound, in_pool_provider, "the child left its parent's pool");
        assert_eq!(stamped, Some(pool_id), "the child was not stamped with the pool");

        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
    }

    /// DB-gated: the follow-up target must be the caller's own child — anything
    /// else refuses, or any session could inject prompts into any other. Its
    /// machine is free: cross-machine delivery is the bus's job.
    #[tokio::test]
    async fn message_child_only_reaches_own_children() {
        let Some(url) =
            crate::routes::gateway::test_db_url("message_child_only_reaches_own_children")
        else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");

        let uid = Uuid::new_v4();
        let stranger = Uuid::new_v4();
        let machine = Uuid::new_v4();
        let other_machine = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, 'mc-test', $2)")
            .bind(uid)
            .bind(format!("kh-{uid}"))
            .execute(&pool)
            .await
            .expect("seed user");
        for m in [machine, other_machine] {
            sqlx::query(
                "INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, $3, $4)",
            )
            .bind(m)
            .bind(uid)
            .bind(m.to_string())
            .bind(format!("kh-{m}"))
            .execute(&pool)
            .await
            .expect("seed machine");
        }
        let parent_id = Uuid::new_v4().to_string();
        let child_id = Uuid::new_v4().to_string();
        let elsewhere_id = Uuid::new_v4().to_string();
        let orphan_id = Uuid::new_v4().to_string();
        for (id, parent, m) in [
            (&parent_id, None::<&str>, machine),
            (&child_id, Some(parent_id.as_str()), machine),
            (&elsewhere_id, Some(parent_id.as_str()), other_machine),
            (&orphan_id, None, machine),
        ] {
            sqlx::query(
                "INSERT INTO sessions (id, parent_id, machine_id, working_dir, user_id, \
                 machine_uuid, adapter_id) VALUES ($1, $2, $3, '/w', $4, $5, 'claude-code')",
            )
            .bind(id)
            .bind(parent)
            .bind(m.to_string())
            .bind(uid)
            .bind(m)
            .execute(&pool)
            .await
            .expect("seed session");
        }
        let parent = Parent {
            session_id: parent_id.clone(),
            machine_uuid: machine,
            working_dir: None,
            user_id: uid,
            permission_mode: None,
        };

        assert_eq!(
            resolve_child_adapter(&pool, uid, &parent, &child_id).await.unwrap(),
            "claude-code"
        );
        assert_eq!(
            resolve_child_adapter(&pool, uid, &parent, &orphan_id).await.unwrap_err().status(),
            StatusCode::FORBIDDEN,
            "a non-child of the caller must refuse"
        );
        assert_eq!(
            resolve_child_adapter(&pool, uid, &parent, &elsewhere_id).await.unwrap(),
            "claude-code",
            "a child on another machine is reachable: the bus routes to ITS machine"
        );
        assert_eq!(
            resolve_child_adapter(&pool, stranger, &parent, &child_id).await.unwrap_err().status(),
            StatusCode::NOT_FOUND,
            "another user's lookup must not even see the session"
        );
        assert_eq!(
            resolve_child_adapter(&pool, uid, &parent, "no-such-session")
                .await
                .unwrap_err()
                .status(),
            StatusCode::NOT_FOUND,
        );

        sqlx::query("DELETE FROM sessions WHERE user_id = $1").bind(uid).execute(&pool).await.ok();
        sqlx::query("DELETE FROM machines WHERE user_id = $1").bind(uid).execute(&pool).await.ok();
        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
    }

    /// DB-gated: a crashed, killed or archived child frees its quota slot, while
    /// a completed-successful child and a still-running one both keep counting.
    #[tokio::test]
    async fn failed_children_free_their_quota_slot() {
        let Some(url) =
            crate::routes::gateway::test_db_url("failed_children_free_their_quota_slot")
        else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");

        let uid = Uuid::new_v4();
        let machine = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, 'q-test', $2)")
            .bind(uid)
            .bind(format!("kh-{uid}"))
            .execute(&pool)
            .await
            .expect("seed user");
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, $3, $4)")
            .bind(machine)
            .bind(uid)
            .bind(machine.to_string())
            .bind(format!("kh-{machine}"))
            .execute(&pool)
            .await
            .expect("seed machine");

        let parent_id = Uuid::new_v4().to_string();
        let running = Uuid::new_v4().to_string();
        let completed = Uuid::new_v4().to_string();
        let crashed = Uuid::new_v4().to_string();
        let killed = Uuid::new_v4().to_string();
        let archived_running = Uuid::new_v4().to_string();
        let archived_completed = Uuid::new_v4().to_string();
        for id in [
            &parent_id,
            &running,
            &completed,
            &crashed,
            &killed,
            &archived_running,
            &archived_completed,
        ] {
            let parent = (*id != parent_id).then(|| parent_id.clone());
            sqlx::query(
                "INSERT INTO sessions (id, parent_id, machine_id, working_dir, user_id, \
                 machine_uuid, adapter_id) VALUES ($1, $2, $3, '/w', $4, $5, 'opencode')",
            )
            .bind(id)
            .bind(parent)
            .bind(machine.to_string())
            .bind(uid)
            .bind(machine)
            .execute(&pool)
            .await
            .expect("seed session");
        }
        let end = |id: &str, reason: serde_json::Value| {
            let id = id.to_owned();
            let pool = pool.clone();
            async move {
                sqlx::query(
                    "INSERT INTO stream_events (session_id, event_type, payload) \
                     VALUES ($1, 'session_ended', $2)",
                )
                .bind(id)
                .bind(json!({ "reason": reason }))
                .execute(&pool)
                .await
                .expect("seed session_ended");
            }
        };
        end(&completed, json!("Completed")).await;
        end(&crashed, json!({ "Crashed": { "detail": "gateway rejected" } })).await;
        end(&killed, json!("Killed")).await;
        end(&archived_completed, json!("Completed")).await;
        sqlx::query("UPDATE sessions SET status = 'archived' WHERE id = ANY($1)")
            .bind(vec![archived_running.clone(), archived_completed.clone()])
            .execute(&pool)
            .await
            .expect("archive two children");

        assert_eq!(
            live_child_count(&pool, &parent_id).await,
            2,
            "running + completed count; crashed, killed and archived are freed"
        );

        sqlx::query("DELETE FROM sessions WHERE user_id = $1").bind(uid).execute(&pool).await.ok();
        sqlx::query("DELETE FROM machines WHERE user_id = $1").bind(uid).execute(&pool).await.ok();
        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
    }

    /// The tree the `archive-child` tests share: `root` with children `a`
    /// (itself parent of `a1`), `b`, a pinned child and another user's row, plus
    /// an `outsider` outside the tree. `root` may hold 4 children.
    struct ArchiveTree {
        pool: sqlx::PgPool,
        state: AppState,
        uid: Uuid,
        stranger: Uuid,
        root_cap: SpawnCapability,
        root: String,
        a: String,
        a1: String,
        b: String,
        pinned: String,
        foreign: String,
        outsider: String,
    }

    impl ArchiveTree {
        async fn seed(test: &str) -> Option<Self> {
            let url = crate::routes::gateway::test_db_url(test)?;
            let pool = sqlx::postgres::PgPoolOptions::new()
                .max_connections(4)
                .connect(&url)
                .await
                .expect("connect test db");
            let (uid, stranger, machine) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
            for (u, name) in [(uid, "ac-test"), (stranger, "ac-stranger")] {
                sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
                    .bind(u)
                    .bind(name)
                    .bind(format!("kh-{u}"))
                    .execute(&pool)
                    .await
                    .expect("seed user");
            }
            sqlx::query(
                "INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, $3, $4)",
            )
            .bind(machine)
            .bind(uid)
            .bind(machine.to_string())
            .bind(format!("kh-{machine}"))
            .execute(&pool)
            .await
            .expect("seed machine");
            let id = || Uuid::new_v4().to_string();
            let (root, a, a1, b, pinned, foreign, outsider) =
                (id(), id(), id(), id(), id(), id(), id());
            for (sid, parent, owner) in [
                (&root, None::<&str>, uid),
                (&a, Some(root.as_str()), uid),
                (&a1, Some(a.as_str()), uid),
                (&b, Some(root.as_str()), uid),
                (&pinned, Some(root.as_str()), uid),
                (&foreign, Some(root.as_str()), stranger),
                (&outsider, None, uid),
            ] {
                sqlx::query(
                    "INSERT INTO sessions (id, parent_id, machine_id, working_dir, user_id, \
                     machine_uuid, adapter_id) VALUES ($1, $2, $3, '/w', $4, $5, 'claude-code')",
                )
                .bind(sid)
                .bind(parent)
                .bind(machine.to_string())
                .bind(owner)
                .bind(machine)
                .execute(&pool)
                .await
                .expect("seed session");
            }
            sqlx::query("UPDATE sessions SET pinned = true WHERE id = $1")
                .bind(&pinned)
                .execute(&pool)
                .await
                .expect("pin");
            let root_cap = cap(&["claude-code"], None, Some(4));
            crate::store::spawn_capabilities::upsert(&pool, &root, &root_cap)
                .await
                .expect("seed root capability");
            let state = AppState::for_test(pool.clone());
            Some(Self {
                pool,
                state,
                uid,
                stranger,
                root_cap,
                root,
                a,
                a1,
                b,
                pinned,
                foreign,
                outsider,
            })
        }

        async fn status(&self, sid: &str) -> String {
            sqlx::query_scalar("SELECT status FROM sessions WHERE id = $1")
                .bind(sid)
                .fetch_one(&self.pool)
                .await
                .expect("status")
        }

        async fn archive(
            &self,
            caller: Uuid,
            from: &str,
            to: &str,
        ) -> Result<ArchiveChildResponse, AppError> {
            archive_descendant(&self.state, caller, from, to).await
        }

        async fn cleanup(self) {
            let users = vec![self.uid, self.stranger];
            sqlx::query("DELETE FROM sessions WHERE user_id = ANY($1)")
                .bind(&users)
                .execute(&self.pool)
                .await
                .ok();
            crate::store::spawn_capabilities::delete(&self.pool, &self.root).await.ok();
            sqlx::query("DELETE FROM machines WHERE user_id = $1")
                .bind(self.uid)
                .execute(&self.pool)
                .await
                .ok();
            sqlx::query("DELETE FROM users WHERE id = ANY($1)")
                .bind(&users)
                .execute(&self.pool)
                .await
                .ok();
        }
    }

    /// DB-gated: `CctuiAgentArchive` reaches only the caller's own descendants;
    /// anything else is a 404 and stays untouched.
    #[tokio::test]
    async fn archive_child_is_a_404_outside_the_callers_tree() {
        let Some(t) = ArchiveTree::seed("archive_child_is_a_404_outside_the_callers_tree").await
        else {
            return;
        };
        for (caller, from, to, why) in [
            (t.uid, &t.a, &t.b, "a sibling"),
            (t.uid, &t.a, &t.root, "its parent"),
            (t.uid, &t.a, &t.a, "itself"),
            (t.uid, &t.root, &t.outsider, "a session outside the tree"),
            (t.uid, &t.root, &t.foreign, "another user's session"),
            (t.stranger, &t.root, &t.a, "a stranger's caller id"),
        ] {
            let status = t.archive(caller, from, to).await.err().map(|e| e.status());
            assert_eq!(status, Some(StatusCode::NOT_FOUND), "archiving {why} must be a 404");
            assert_ne!(t.status(to).await, "archived", "{why} was archived anyway");
        }
        t.cleanup().await;
    }

    /// DB-gated: a child's completion reaches only its own parent, owned by the
    /// same user, and only while that parent can take a turn.
    #[tokio::test]
    async fn a_child_completion_is_only_offered_to_its_own_live_parent() {
        let Some(t) =
            ArchiveTree::seed("a_child_completion_is_only_offered_to_its_own_live_parent").await
        else {
            return;
        };
        let code = |r: Result<String, AppError>| r.err().map(|e| e.status());
        assert_eq!(
            code(deliver_completion(&t.state, t.stranger, &t.a, "done").await),
            Some(StatusCode::NOT_FOUND),
            "another user's machine key cannot post a completion"
        );
        assert_eq!(
            code(deliver_completion(&t.state, t.uid, &t.foreign, "done").await),
            Some(StatusCode::NOT_FOUND),
            "a child owned by someone else is unknown"
        );
        assert_eq!(
            code(deliver_completion(&t.state, t.uid, &t.outsider, "done").await),
            Some(StatusCode::CONFLICT),
            "a root session has no parent to report to"
        );
        assert_eq!(
            code(deliver_completion(&t.state, t.uid, &t.a, "   ").await),
            Some(StatusCode::BAD_REQUEST)
        );
        let huge = "x".repeat(MAX_COMPLETION_BYTES + 1);
        assert_eq!(
            code(deliver_completion(&t.state, t.uid, &t.a, &huge).await),
            Some(StatusCode::PAYLOAD_TOO_LARGE)
        );
        let reached = deliver_completion(&t.state, t.uid, &t.a1, "the grandchild is done").await;
        assert!(
            matches!(&reached, Ok(parent) if *parent == t.a)
                || code(reached) == Some(StatusCode::SERVICE_UNAVAILABLE),
            "an authorized completion must go to the child's own parent (or fail only on \
             delivery, with no daemon connected in the test)"
        );
        sqlx::query("UPDATE sessions SET status = 'archived' WHERE id = $1")
            .bind(&t.root)
            .execute(&t.pool)
            .await
            .expect("archive root");
        assert_eq!(
            code(deliver_completion(&t.state, t.uid, &t.a, "done").await),
            Some(StatusCode::CONFLICT),
            "an archived parent is never offered the turn"
        );
        t.cleanup().await;
    }

    /// DB-gated: a pin is the human's mark; an agent never overrides it.
    #[tokio::test]
    async fn archive_child_refuses_a_pinned_child() {
        let Some(t) = ArchiveTree::seed("archive_child_refuses_a_pinned_child").await else {
            return;
        };
        let pin = t.archive(t.uid, &t.root, &t.pinned).await;
        assert_eq!(pin.err().map(|e| e.status()), Some(StatusCode::CONFLICT));
        assert_ne!(t.status(&t.pinned).await, "archived", "a pin is never overridden");
        t.cleanup().await;
    }

    /// DB-gated: a grandparent may release a grandchild, and archiving a child
    /// cascades to its own children and reports every archived row.
    #[tokio::test]
    async fn archive_child_cascades_and_reaches_grandchildren() {
        let Some(t) = ArchiveTree::seed("archive_child_cascades_and_reaches_grandchildren").await
        else {
            return;
        };
        let grand = t.archive(t.uid, &t.root, &t.a1).await.expect("grandchild");
        assert_eq!(grand.archived, vec![t.a1.clone()], "a grandparent may release a grandchild");

        let released = t.archive(t.uid, &t.root, &t.a).await.expect("direct child");
        let mut expected = vec![t.a.clone(), t.a1.clone()];
        expected.sort();
        assert_eq!(released.archived, expected, "the cascade reports every archived row");
        assert_eq!(t.status(&t.a).await, "archived");
        t.cleanup().await;
    }

    /// DB-gated: archiving hands the slot back, so a spawn refused at the cap
    /// goes through, and `CctuiUsage` reports the same count.
    #[tokio::test]
    async fn archive_child_frees_a_slot_for_a_refused_spawn() {
        let Some(t) = ArchiveTree::seed("archive_child_frees_a_slot_for_a_refused_spawn").await
        else {
            return;
        };
        assert_eq!(slots_used(&t.pool, &t.root).await, 4, "a, b, pinned and foreign count");
        assert_eq!(
            authorize(Some(&t.root_cap), &req("claude-code", None), &usage(4)),
            Err(Denied::TooManyChildren { max: 4 }),
        );

        let released = t.archive(t.uid, &t.root, &t.a).await.expect("direct child");
        assert_eq!(released.children_used, 3);
        assert_eq!(released.max_children, Some(4));
        assert!(
            authorize(Some(&t.root_cap), &req("claude-code", None), &usage(released.children_used))
                .is_ok(),
            "the freed slot lets the refused spawn through"
        );
        assert_eq!(
            child_slots(&t.state, &t.root).await,
            Some(ChildSlots { used: 3, max: Some(4), depth_left: None }),
            "CctuiUsage reports the same count"
        );
        t.cleanup().await;
    }

    /// DB-gated: reservations across a two-level tree are summed from the
    /// ledger under the root, and a released reservation frees its share.
    #[tokio::test]
    async fn reservations_sum_across_the_tree_under_the_root_ceiling() {
        let Some(url) = crate::routes::gateway::test_db_url("spawn_tree_reservations") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let root_id = Uuid::new_v4().to_string();
        let root =
            SpawnCapability { max_tree_budget_usd: Some(5.0), ..cap(&["codex"], Some(2.0), None) };
        let (a, child_cap) = reserve_child(
            &pool,
            &root_id,
            Some(&root),
            &req("codex", None),
            usage(0),
            &Uuid::new_v4().to_string(),
        )
        .await
        .expect("first child");
        assert_eq!(a.budget_usd, Some(2.0));
        assert_eq!(child_cap.tree_root.as_deref(), Some(root_id.as_str()));

        let child_id = Uuid::new_v4().to_string();
        let grandchild_key = Uuid::new_v4().to_string();
        let (a, _) = reserve_child(
            &pool,
            &child_id,
            Some(&child_cap),
            &req("codex", None),
            usage(0),
            &grandchild_key,
        )
        .await
        .expect("grandchild");
        assert_eq!(a.budget_usd, Some(2.0));

        let (a, _) = reserve_child(
            &pool,
            &root_id,
            Some(&root),
            &req("codex", None),
            usage(1),
            &Uuid::new_v4().to_string(),
        )
        .await
        .expect("clamped to the remainder");
        assert_eq!(a.budget_usd, Some(1.0));
        assert!((tree_granted_usd(&pool, &root_id).await.unwrap() - 5.0).abs() < 1e-9);

        let err = reserve_child(
            &pool,
            &child_id,
            Some(&child_cap),
            &req("codex", None),
            usage(1),
            &Uuid::new_v4().to_string(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.status(), StatusCode::FORBIDDEN, "the tree is spent");

        release_child(&pool, &grandchild_key).await;
        assert!((tree_granted_usd(&pool, &root_id).await.unwrap() - 3.0).abs() < 1e-9);

        sqlx::query("DELETE FROM session_spawn_capabilities WHERE capability->>'tree_root' = $1")
            .bind(&root_id)
            .execute(&pool)
            .await
            .ok();
        sqlx::query("DELETE FROM spawn_tree_grants WHERE root_id = $1")
            .bind(&root_id)
            .execute(&pool)
            .await
            .ok();
    }

    /// DB-gated: a reserved child that has not registered yet still occupies
    /// its slot, so back-to-back spawns cannot overshoot `max_children`.
    #[tokio::test]
    async fn unregistered_reservations_count_against_max_children() {
        let Some(url) = crate::routes::gateway::test_db_url("spawn_child_pending_slots") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let parent_id = Uuid::new_v4().to_string();
        let parent = cap(&["codex"], Some(1.0), Some(2));
        for _ in 0..2 {
            let key = Uuid::new_v4().to_string();
            reserve_child(&pool, &parent_id, Some(&parent), &req("codex", None), usage(0), &key)
                .await
                .expect("within the child cap");
        }
        let key = Uuid::new_v4().to_string();
        let err =
            reserve_child(&pool, &parent_id, Some(&parent), &req("codex", None), usage(0), &key)
                .await
                .unwrap_err();
        assert_eq!(err.status(), StatusCode::FORBIDDEN);
        assert!(err.message().contains("maximum of 2"), "{}", err.message());

        sqlx::query("DELETE FROM session_spawn_capabilities WHERE capability->>'tree_root' = $1")
            .bind(&parent_id)
            .execute(&pool)
            .await
            .ok();
        sqlx::query("DELETE FROM spawn_tree_grants WHERE root_id = $1")
            .bind(&parent_id)
            .execute(&pool)
            .await
            .ok();
    }

    fn parent_of(session_id: &str, owner: Uuid) -> Parent {
        Parent {
            session_id: session_id.to_owned(),
            machine_uuid: Uuid::new_v4(),
            working_dir: None,
            user_id: owner,
            permission_mode: None,
        }
    }

    /// DB-gated: a spawn grantee's session belongs to the machine owner but runs
    /// on the grantee's account, and so does its tree.
    #[tokio::test]
    async fn a_grantee_trees_child_runs_on_the_grantees_account() {
        let Some(url) = crate::routes::gateway::test_db_url(
            "a_grantee_trees_child_runs_on_the_grantees_account",
        ) else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let owner = Uuid::new_v4();
        let grantee = Uuid::new_v4();
        seed_accounts(&pool, owner, "machine-owner", &[]).await;
        let mine = seed_accounts(&pool, grantee, "grantee", &[("g-claude", "anthropic")]).await;
        let (account, provider) = mine[0];
        let state = crate::state::AppState::for_test(pool.clone());
        let parent_id = Uuid::new_v4().to_string();
        crate::routes::gateway::mint_session_env(
            &state,
            grantee,
            &format!("g-claude-{account}"),
            crate::routes::gateway::Family::Anthropic,
            &parent_id,
        )
        .await
        .expect("bind the parent to the grantee's account");

        let child_key = Uuid::new_v4().to_string();
        child_account_env(
            &state,
            &parent_of(&parent_id, owner),
            "claude-code",
            crate::routes::gateway::Family::Anthropic,
            None,
            &child_key,
        )
        .await
        .expect("the child mints on the account its parent runs on");
        let bound: (Uuid, Option<Uuid>) =
            sqlx::query_as("SELECT account_id, user_id FROM session_tokens WHERE session_id = $1")
                .bind(&child_key)
                .fetch_one(&pool)
                .await
                .expect("child token");
        assert_eq!(bound, (provider, Some(grantee)));

        for u in [owner, grantee] {
            sqlx::query("DELETE FROM users WHERE id = $1").bind(u).execute(&pool).await.ok();
        }
    }

    /// DB-gated: a child is attributed to whoever spawned its parent, else to
    /// the parent's owner.
    #[tokio::test]
    async fn a_child_is_attributed_to_the_human_behind_its_tree() {
        let Some(url) = crate::routes::gateway::test_db_url(
            "a_child_is_attributed_to_the_human_behind_its_tree",
        ) else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
        let owner = Uuid::new_v4();
        let grantee = Uuid::new_v4();
        seed_accounts(&pool, owner, "attr-owner", &[]).await;
        seed_accounts(&pool, grantee, "attr-grantee", &[]).await;
        let mut parents = Vec::new();
        for spawner in [Some(grantee), None] {
            let sid = Uuid::new_v4().to_string();
            sqlx::query(
                "INSERT INTO sessions (id, machine_id, working_dir, user_id, spawned_by) \
                 VALUES ($1, 'm', '/w', $2, $3)",
            )
            .bind(&sid)
            .bind(owner)
            .bind(spawner)
            .execute(&pool)
            .await
            .expect("seed parent");
            parents.push(sid);
        }
        assert_eq!(child_spawner(&pool, &parent_of(&parents[0], owner)).await, grantee);
        assert_eq!(child_spawner(&pool, &parent_of(&parents[1], owner)).await, owner);

        for sid in &parents {
            sqlx::query("DELETE FROM sessions WHERE id = $1").bind(sid).execute(&pool).await.ok();
        }
        for u in [owner, grantee] {
            sqlx::query("DELETE FROM users WHERE id = $1").bind(u).execute(&pool).await.ok();
        }
    }
}
