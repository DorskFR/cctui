//! `POST /api/v1/daemon/sessions/{id}/spawn-child` — the server side of the
//! daemon's `CctuiAgent` tool.
//!
//! A session asks its daemon to spawn a subagent; the daemon relays the request
//! here with its machine key. The server is the only place the decision is made:
//! it reads the calling session's [`SpawnCapability`] — set by whoever launched
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

use cctui_proto::adapter::{AdapterCommand, AdapterId, PermissionMode, SessionSpec};
use cctui_proto::api::{ApiError, SpawnCapability, SpawnChildRequest, SpawnChildResponse};
use cctui_proto::ws::DaemonFrameDown;
use uuid::Uuid;

/// The parent session's launch context, everything a child inherits.
struct Parent {
    session_id: String,
    machine_uuid: Uuid,
    working_dir: Option<String>,
    user_id: Uuid,
    permission_mode: Option<PermissionMode>,
}

/// What the parent's tree has already consumed when a child is requested.
#[derive(Debug, Clone, Copy, Default)]
pub struct Usage {
    pub live_children: u32,
    /// Sum of budgets already granted to descendants of the tree root.
    pub tree_granted_usd: f64,
    /// The parent's live posture, when known.
    pub parent_mode: Option<PermissionMode>,
}

/// A spawn request that cleared the capability check.
#[derive(Debug, PartialEq)]
pub struct Authorized {
    pub adapter: String,
    pub budget_usd: Option<f64>,
    pub permission_mode: PermissionMode,
}

/// Why a `CctuiAgent` call was refused. Rendered verbatim into the tool result,
/// so each variant names the limit that stopped it.
#[derive(Debug, PartialEq)]
pub enum Denied {
    NoCapability,
    Adapter { requested: String, allowed: Vec<String> },
    Budget { requested: f64, max: Option<f64> },
    TooManyChildren { max: u32 },
    Depth,
    PermissionMode { requested: PermissionMode, max: PermissionMode },
    TreeBudget { requested: f64, remaining: f64 },
    BadRequest(String),
}

impl std::fmt::Display for Denied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoCapability => {
                f.write_str("this session has no spawn capability — CctuiAgent is disabled for it")
            }
            Self::Adapter { requested, allowed } => write!(
                f,
                "adapter {requested:?} is not permitted for this session (allowed: {})",
                allowed.join(", ")
            ),
            Self::Budget { requested, max } => match max {
                Some(max) => {
                    write!(f, "budget_usd {requested} exceeds this session's ceiling {max}")
                }
                None => write!(
                    f,
                    "budget_usd {requested} requested but this session may not set a dollar budget"
                ),
            },
            Self::TooManyChildren { max } => {
                write!(f, "this session already spawned its maximum of {max} children")
            }
            Self::Depth => f.write_str("this session is at its maximum spawn depth"),
            Self::PermissionMode { requested, max } => write!(
                f,
                "permission_mode {} is more permissive than this session's {}",
                requested.normalized_label(),
                max.normalized_label()
            ),
            Self::TreeBudget { requested, remaining } => write!(
                f,
                "budget_usd {requested} exceeds the {remaining} left in this spawn tree's budget"
            ),
            Self::BadRequest(msg) => f.write_str(msg),
        }
    }
}

/// Decide whether `req` is permitted by `cap`, and with what budget.
///
/// Pure and fail-closed: an absent capability, an unlisted adapter, a budget
/// over the per-child or remaining tree ceiling, a child count at the cap, an
/// exhausted depth, or a posture above the parent's all deny. A call that names
/// no budget inherits the smaller of the ceiling and what the tree has left; one
/// that names no posture inherits the parent's.
pub fn authorize(
    cap: Option<&SpawnCapability>,
    req: &SpawnChildRequest,
    usage: &Usage,
) -> Result<Authorized, Denied> {
    let Some(cap) = cap.filter(|c| !c.is_empty()) else {
        return Err(Denied::NoCapability);
    };
    let adapter = req.adapter.trim();
    if adapter.is_empty() {
        return Err(Denied::BadRequest("adapter is required".into()));
    }
    if req.prompt.trim().is_empty() {
        return Err(Denied::BadRequest("prompt is required".into()));
    }
    if !cap.allows_adapter(adapter) {
        return Err(Denied::Adapter {
            requested: adapter.to_owned(),
            allowed: cap.adapters.clone(),
        });
    }
    if cap.max_depth == Some(0) {
        return Err(Denied::Depth);
    }
    if let Some(max) = cap.max_children
        && usage.live_children >= max
    {
        return Err(Denied::TooManyChildren { max });
    }
    let ceiling = match (cap.max_permission_mode, usage.parent_mode) {
        (Some(c), Some(p)) => PermissionMode::stricter(p, c),
        (c, p) => p.or(c).unwrap_or(PermissionMode::Ask),
    };
    let permission_mode = match req.permission_mode {
        Some(m) if !m.within(ceiling) => {
            return Err(Denied::PermissionMode { requested: m, max: ceiling });
        }
        Some(m) => m,
        None => ceiling,
    };
    let budget = match req.budget_usd {
        None => cap.max_budget_usd,
        Some(b) if !b.is_finite() || b <= 0.0 => {
            return Err(Denied::BadRequest("budget_usd must be a positive number".into()));
        }
        Some(b) => match cap.max_budget_usd {
            Some(max) if b <= max => Some(b),
            max => return Err(Denied::Budget { requested: b, max }),
        },
    };
    let budget = match cap.max_tree_budget_usd {
        None => budget,
        Some(tree) => {
            let remaining = (tree - usage.tree_granted_usd).max(0.0);
            match (req.budget_usd, budget) {
                (Some(b), _) if b > remaining => {
                    return Err(Denied::TreeBudget { requested: b, remaining });
                }
                _ if remaining <= 0.0 => {
                    return Err(Denied::TreeBudget { requested: budget.unwrap_or(0.0), remaining });
                }
                (_, Some(b)) => Some(b.min(remaining)),
                (_, None) => Some(remaining),
            }
        }
    };
    Ok(Authorized { adapter: adapter.to_owned(), budget_usd: budget, permission_mode })
}

use crate::state::AppState;

fn deny(code: StatusCode, msg: impl Into<String>) -> (StatusCode, Json<ApiError>) {
    (code, Json(ApiError { error: msg.into() }))
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
        .ok_or_else(|| deny(StatusCode::UNAUTHORIZED, "machine key required"))?;
    let ctx = state
        .auth_config
        .validate(token)
        .await
        .ok_or_else(|| deny(StatusCode::UNAUTHORIZED, "invalid machine key"))?;
    if ctx.machine_id.is_none() {
        return Err(deny(StatusCode::FORBIDDEN, "machine token required"));
    }
    Ok(ctx.user_id)
}

async fn load_parent(
    state: &AppState,
    session_id: &str,
    caller: Uuid,
) -> Result<Parent, (StatusCode, Json<ApiError>)> {
    let row: Option<(Option<Uuid>, Option<String>, Option<Uuid>, Option<String>)> = sqlx::query_as(
        "SELECT machine_uuid, working_dir, user_id, permission_mode FROM sessions WHERE id = $1",
    )
    .bind(session_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!(%session_id, "db error (spawn-child parent): {e}");
        deny(StatusCode::INTERNAL_SERVER_ERROR, "database error")
    })?;
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

/// The account identity the parent is bound to, resolved so the child can mint
/// its own gateway env under the same account for ITS family (a claude parent
/// spawning an opencode child crosses families).
async fn parent_account_name(state: &AppState, session_id: &str) -> Option<String> {
    sqlx::query_scalar(
        "SELECT a.name FROM session_tokens st \
         JOIN account_providers ap ON ap.id = st.account_id \
         JOIN accounts a ON a.id = ap.account_id \
         WHERE st.session_id = $1 \
         ORDER BY (st.revoked_at IS NULL) DESC, st.created_at DESC LIMIT 1",
    )
    .bind(session_id)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten()
}

/// Reservations for `parent_id` whose child has not registered yet: its
/// capability is still keyed by the spawn key and no session row exists.
async fn pending_child_count(
    exec: impl sqlx::PgExecutor<'_>,
    parent_id: &str,
) -> Result<u32, sqlx::Error> {
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM spawn_tree_grants g \
         WHERE g.parent_id = $1 \
         AND EXISTS (SELECT 1 FROM session_spawn_capabilities c WHERE c.session_id = g.child_id) \
         AND NOT EXISTS (SELECT 1 FROM sessions s WHERE s.id = g.child_id)",
    )
    .bind(parent_id)
    .fetch_one(exec)
    .await?;
    Ok(u32::try_from(n).unwrap_or(u32::MAX))
}

/// Sum of the budgets ever granted to descendants of `root`.
async fn tree_granted_usd(exec: impl sqlx::PgExecutor<'_>, root: &str) -> Result<f64, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT COALESCE(SUM(budget_usd), 0)::float8 FROM spawn_tree_grants WHERE root_id = $1",
    )
    .bind(root)
    .fetch_one(exec)
    .await
}

/// Return a reservation whose child never launched to the tree budget.
async fn release_child(pool: &sqlx::PgPool, child_key: &str) {
    let released = async {
        crate::store::spawn_capabilities::delete(pool, child_key).await?;
        sqlx::query("DELETE FROM spawn_tree_grants WHERE child_id = $1")
            .bind(child_key)
            .execute(pool)
            .await
            .map(|_| ())
    };
    if let Err(e) = released.await {
        tracing::error!(child = %child_key, error = %e, "spawn-child reservation release failed");
    }
}

/// Count children, authorize `req` and persist the child's capability in one
/// transaction under a per-tree advisory lock, so concurrent spawns cannot both
/// claim the same child slot or slice of the tree budget.
async fn reserve_child(
    pool: &sqlx::PgPool,
    parent_id: &str,
    cap: Option<&SpawnCapability>,
    req: &SpawnChildRequest,
    mut usage: Usage,
    child_key: &str,
) -> Result<(Authorized, SpawnCapability), (StatusCode, Json<ApiError>)> {
    let db_err = |e: sqlx::Error| {
        tracing::error!(parent = %parent_id, error = %e, "spawn-child reservation failed");
        deny(StatusCode::INTERNAL_SERVER_ERROR, "database error")
    };
    let root = cap.and_then(|c| c.tree_root.clone()).unwrap_or_else(|| parent_id.to_owned());
    let mut tx = pool.begin().await.map_err(db_err)?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
        .bind(format!("spawn-tree:{root}"))
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
    usage.tree_granted_usd = tree_granted_usd(&mut *tx, &root).await.map_err(db_err)?;
    usage.live_children = live_child_count(&mut *tx, parent_id)
        .await
        .saturating_add(pending_child_count(&mut *tx, parent_id).await.map_err(db_err)?);
    let authorized = authorize(cap, req, &usage).map_err(|d| match d {
        Denied::BadRequest(_) => deny(StatusCode::BAD_REQUEST, d.to_string()),
        _ => deny(StatusCode::FORBIDDEN, d.to_string()),
    })?;
    let child_cap = cap.map_or_else(SpawnCapability::machine_default, |c| {
        c.inherited(parent_id, authorized.budget_usd, Some(authorized.permission_mode))
    });
    crate::store::spawn_capabilities::upsert(&mut *tx, child_key, &child_cap)
        .await
        .map_err(db_err)?;
    sqlx::query(
        "INSERT INTO spawn_tree_grants (root_id, parent_id, child_id, budget_usd) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(&root)
    .bind(parent_id)
    .bind(child_key)
    .bind(authorized.budget_usd.unwrap_or(0.0))
    .execute(&mut *tx)
    .await
    .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok((authorized, child_cap))
}

/// Children counting against the parent's spawn quota: every child except those
/// that ended in failure. A child that emitted a terminal `session_ended` whose
/// reason is anything but `Completed` (crashed, killed, adapter error) has freed
/// its slot, so the parent can respawn a replacement. Still-running and
/// completed-successful children both count.
async fn live_child_count(exec: impl sqlx::PgExecutor<'_>, parent_id: &str) -> u32 {
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sessions s \
         WHERE s.parent_id = $1 \
         AND NOT EXISTS ( \
             SELECT 1 FROM stream_events e \
             WHERE e.session_id = s.id AND e.event_type = 'session_ended' \
             AND e.payload->>'reason' <> 'Completed' \
         )",
    )
    .bind(parent_id)
    .fetch_one(exec)
    .await
    .unwrap_or(0);
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Mint the child's gateway env and resolve its model under the parent's account.
/// A child of an unbound parent is unbound too (empty env, model as requested).
async fn child_account_env(
    state: &AppState,
    parent: &Parent,
    family: crate::routes::gateway::Family,
    requested_model: Option<&str>,
    child_key: &str,
) -> Result<
    (std::collections::BTreeMap<String, String>, Option<String>),
    (StatusCode, Json<ApiError>),
> {
    let mut model = requested_model.map(str::trim).filter(|m| !m.is_empty()).map(str::to_owned);
    let Some(account) = parent_account_name(state, &parent.session_id).await else {
        return Ok((std::collections::BTreeMap::new(), model));
    };
    if model.is_some() || family == crate::routes::gateway::Family::Fireworks {
        let resolved = crate::routes::gateway::resolve_account_model(
            state,
            parent.user_id,
            &account,
            family,
            model.as_deref().unwrap_or_default(),
        )
        .await;
        model = (!resolved.is_empty()).then_some(resolved);
    }
    match crate::routes::gateway::mint_session_env(
        state,
        parent.user_id,
        &account,
        family,
        child_key,
    )
    .await
    {
        Ok(env) => Ok((env, model)),
        Err(e) => {
            let why = match e {
                crate::routes::gateway::MintSessionEnvError::NoAccount => {
                    "the parent's account no longer exists".to_owned()
                }
                crate::routes::gateway::MintSessionEnvError::NoProviderForFamily(f) => {
                    format!("the parent's account has no {} provider", f.label())
                }
                crate::routes::gateway::MintSessionEnvError::Db(err) => {
                    tracing::error!(parent = %parent.session_id, "spawn-child mint failed: {err}");
                    "database error".to_owned()
                }
            };
            Err(deny(
                StatusCode::CONFLICT,
                format!("could not provision a {} child: {why}", family.label()),
            ))
        }
    }
}

/// The caller's capability, from the in-memory cache or the durable table it
/// fronts. A DB error denies rather than grants — the check stays fail-closed.
async fn capability_for(state: &AppState, session_id: &str) -> Option<SpawnCapability> {
    if let Some(cap) = state.spawn_capabilities.get(session_id) {
        return Some(cap.clone());
    }
    match crate::store::spawn_capabilities::get(&state.pool, session_id).await {
        Ok(Some(cap)) => {
            state.spawn_capabilities.insert(session_id.to_owned(), cap.clone());
            Some(cap)
        }
        Ok(None) => None,
        Err(e) => {
            tracing::error!(%session_id, error = %e, "spawn-capability lookup failed — denying");
            None
        }
    }
}

/// `POST /api/v1/daemon/sessions/{id}/spawn-child`.
pub async fn spawn_child(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(session_id): Path<String>,
    Json(req): Json<SpawnChildRequest>,
) -> Result<Json<SpawnChildResponse>, (StatusCode, Json<ApiError>)> {
    let caller = machine_user(&state, &headers).await?;
    let parent = load_parent(&state, &session_id, caller).await?;

    let cap = capability_for(&state, &session_id).await;
    let usage = Usage { parent_mode: parent.permission_mode, ..Usage::default() };
    let child_id = Uuid::new_v4();
    let child_key = child_id.to_string();
    // Persisted before the frame: the child pulls its gateway env the moment it
    // launches, and a missing row there would hand it the unclamped default.
    let (authorized, child_cap) =
        reserve_child(&state.pool, &session_id, cap.as_ref(), &req, usage, &child_key).await?;
    let family = crate::routes::gateway::Family::from_adapter(&authorized.adapter);
    let (mut env, model) =
        match child_account_env(&state, &parent, family, req.model.as_deref(), &child_key).await {
            Ok(v) => v,
            Err(e) => {
                release_child(&state.pool, &child_key).await;
                return Err(e);
            }
        };
    if let Some(profile) = req.agent_profile.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        env.insert(AGENT_PROFILE_ENV.to_owned(), profile.to_owned());
    }

    // A child never self-selects Fast: it inherits the bound account's default,
    // else the standard tier.
    let service_tier = if family == crate::routes::gateway::Family::Openai {
        let account_settings =
            crate::routes::gateway::resolve_session_settings(&state, &child_key).await;
        Some(crate::settings_catalog::codex::resolve_service_tier(None, account_settings.as_ref()))
    } else {
        None
    };
    let spec = SessionSpec {
        adapter_id: AdapterId::new(&authorized.adapter),
        working_dir: req
            .cwd
            .clone()
            .filter(|c| !c.trim().is_empty())
            .or_else(|| parent.working_dir.clone()),
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
    state.spawn_capabilities.insert(child_key.clone(), child_cap);

    let frame = DaemonFrameDown::Command {
        adapter_id: authorized.adapter.clone(),
        command: Box::new(AdapterCommand::Spawn {
            spec,
            command_id: Some(child_id),
            session_id: Some(child_id),
        }),
    };
    if let Err(err) =
        state.bus.command_daemon_for_session(parent.machine_uuid, &parent.session_id, frame).await
    {
        state.spawn_capabilities.remove(&child_key);
        release_child(&state.pool, &child_key).await;
        return Err(deny(
            StatusCode::SERVICE_UNAVAILABLE,
            format!("could not reach the daemon: {err}"),
        ));
    }

    // The child's dollar budget is session-scoped, so it rides the in-memory
    // per-session map the gateway overlays onto the account's soft limits.
    if let Some(budget) = authorized.budget_usd {
        state.session_usd_budgets.insert(child_key.clone(), budget);
    }
    tracing::info!(
        parent = %session_id,
        child = %child_key,
        adapter = %authorized.adapter,
        budget = ?authorized.budget_usd,
        "CctuiAgent child spawned",
    );
    Ok(Json(SpawnChildResponse { session_id: child_key, budget_usd: authorized.budget_usd }))
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
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ApiError>)> {
    let caller = machine_user(&state, &headers).await?;
    let parent = load_parent(&state, &session_id, caller).await?;
    if req.prompt.trim().is_empty() {
        return Err(deny(StatusCode::BAD_REQUEST, "prompt is required"));
    }
    let child = req.session_id.trim();
    if child.is_empty() {
        return Err(deny(StatusCode::BAD_REQUEST, "session_id is required"));
    }
    let adapter_id = resolve_child_adapter(&state.pool, caller, &parent, child).await?;
    let frame = DaemonFrameDown::Command {
        adapter_id,
        command: Box::new(AdapterCommand::SendMessage {
            local_id: child.to_owned(),
            text: req.prompt.clone(),
        }),
    };
    state.bus.command_daemon_for_session(parent.machine_uuid, child, frame).await.map_err(
        |err| deny(StatusCode::SERVICE_UNAVAILABLE, format!("could not reach the daemon: {err}")),
    )?;
    tracing::info!(parent = %session_id, %child, "CctuiAgent follow-up relayed");
    Ok(Json(serde_json::json!({})))
}

/// The follow-up target's adapter, only if `child` really is `parent`'s child
/// on `parent`'s machine and owned by `caller`. Everything else refuses.
async fn resolve_child_adapter(
    pool: &sqlx::PgPool,
    caller: Uuid,
    parent: &Parent,
    child: &str,
) -> Result<String, (StatusCode, Json<ApiError>)> {
    let row: Option<(Option<String>, Option<Uuid>, Option<String>)> = sqlx::query_as(
        "SELECT parent_id, machine_uuid, adapter_id FROM sessions WHERE id = $1 AND user_id = $2",
    )
    .bind(child)
    .bind(caller)
    .fetch_optional(pool)
    .await
    .map_err(|e| {
        tracing::error!(%child, "db error (message-child): {e}");
        deny(StatusCode::INTERNAL_SERVER_ERROR, "database error")
    })?;
    let Some((child_parent, child_machine, adapter_id)) = row else {
        return Err(deny(StatusCode::NOT_FOUND, "child session not found"));
    };
    if child_parent.as_deref() != Some(parent.session_id.as_str()) {
        return Err(deny(StatusCode::FORBIDDEN, "session is not a child of this session"));
    }
    if child_machine != Some(parent.machine_uuid) {
        return Err(deny(StatusCode::CONFLICT, "child session is not on this machine"));
    }
    adapter_id
        .filter(|a| !a.is_empty())
        .ok_or_else(|| deny(StatusCode::CONFLICT, "child session has no adapter"))
}

/// Env key the opencode adapter reads to select an agent profile.
pub const AGENT_PROFILE_ENV: &str = "CCTUI_OPENCODE_AGENT";

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

    #[test]
    fn no_capability_denies() {
        assert_eq!(authorize(None, &req("opencode", None), &usage(0)), Err(Denied::NoCapability));
    }

    #[test]
    fn empty_adapter_list_denies_like_no_capability() {
        let cap = cap(&[], Some(1.0), None);
        assert_eq!(
            authorize(Some(&cap), &req("opencode", None), &usage(0)),
            Err(Denied::NoCapability)
        );
    }

    #[test]
    fn unlisted_adapter_denies_and_names_the_allowed_set() {
        let cap = cap(&["opencode"], Some(1.0), None);
        let err = authorize(Some(&cap), &req("claude-code", None), &usage(0)).unwrap_err();
        assert_eq!(
            err,
            Denied::Adapter {
                requested: "claude-code".into(),
                allowed: vec!["opencode".to_owned()]
            }
        );
        assert!(err.to_string().contains("opencode"));
    }

    #[test]
    fn listed_adapter_allows_and_inherits_the_ceiling_budget() {
        let cap = cap(&["opencode", "codex"], Some(2.5), None);
        let ok = authorize(Some(&cap), &req("opencode", None), &usage(0)).unwrap();
        assert_eq!(
            ok,
            Authorized {
                adapter: "opencode".into(),
                budget_usd: Some(2.5),
                permission_mode: PermissionMode::Ask,
            }
        );
    }

    #[test]
    fn budget_over_the_ceiling_denies() {
        let cap = cap(&["opencode"], Some(2.0), None);
        assert_eq!(
            authorize(Some(&cap), &req("opencode", Some(5.0)), &usage(0)),
            Err(Denied::Budget { requested: 5.0, max: Some(2.0) })
        );
    }

    #[test]
    fn budget_requested_without_a_ceiling_denies() {
        let cap = cap(&["opencode"], None, None);
        assert_eq!(
            authorize(Some(&cap), &req("opencode", Some(0.5)), &usage(0)),
            Err(Denied::Budget { requested: 0.5, max: None })
        );
    }

    #[test]
    fn budget_at_the_ceiling_is_allowed() {
        let cap = cap(&["opencode"], Some(2.0), None);
        assert_eq!(
            authorize(Some(&cap), &req("opencode", Some(2.0)), &usage(0)).unwrap().budget_usd,
            Some(2.0)
        );
    }

    #[test]
    fn nonpositive_or_nonfinite_budget_is_a_bad_request() {
        let cap = cap(&["opencode"], Some(2.0), None);
        assert!(matches!(
            authorize(Some(&cap), &req("opencode", Some(0.0)), &usage(0)),
            Err(Denied::BadRequest(_))
        ));
        assert!(matches!(
            authorize(Some(&cap), &req("opencode", Some(f64::NAN)), &usage(0)),
            Err(Denied::BadRequest(_))
        ));
    }

    #[test]
    fn child_cap_denies_once_reached() {
        let cap = cap(&["opencode"], Some(1.0), Some(2));
        assert!(authorize(Some(&cap), &req("opencode", None), &usage(1)).is_ok());
        assert_eq!(
            authorize(Some(&cap), &req("opencode", None), &usage(2)),
            Err(Denied::TooManyChildren { max: 2 })
        );
    }

    #[test]
    fn machine_default_authorizes_every_known_adapter() {
        let cap = SpawnCapability::machine_default();
        for adapter in cctui_proto::adapter::KNOWN_ADAPTERS {
            let ok = authorize(Some(&cap), &req(adapter, None), &usage(0)).unwrap();
            assert_eq!(
                ok,
                Authorized {
                    adapter: (*adapter).to_owned(),
                    budget_usd: Some(cctui_proto::api::DEFAULT_CHILD_BUDGET_USD),
                    permission_mode: PermissionMode::Ask,
                }
            );
        }
        assert_eq!(
            authorize(Some(&cap), &req("claude-code", Some(1.5)), &usage(0)).unwrap().budget_usd,
            Some(1.5)
        );
        assert!(matches!(
            authorize(Some(&cap), &req("claude-code", Some(1_000.0)), &usage(0)),
            Err(Denied::Budget { .. })
        ));
    }

    /// DB-gated: the follow-up target must be the caller's own child on the
    /// same machine — anything else refuses, or any session could inject
    /// prompts into any other.
    #[tokio::test]
    async fn message_child_only_reaches_own_children_on_the_same_machine() {
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
            resolve_child_adapter(&pool, uid, &parent, &orphan_id).await.unwrap_err().0,
            StatusCode::FORBIDDEN,
            "a non-child of the caller must refuse"
        );
        assert_eq!(
            resolve_child_adapter(&pool, uid, &parent, &elsewhere_id).await.unwrap_err().0,
            StatusCode::CONFLICT,
            "a child on another machine must refuse"
        );
        assert_eq!(
            resolve_child_adapter(&pool, stranger, &parent, &child_id).await.unwrap_err().0,
            StatusCode::NOT_FOUND,
            "another user's lookup must not even see the session"
        );
        assert_eq!(
            resolve_child_adapter(&pool, uid, &parent, "no-such-session").await.unwrap_err().0,
            StatusCode::NOT_FOUND,
        );

        sqlx::query("DELETE FROM sessions WHERE user_id = $1").bind(uid).execute(&pool).await.ok();
        sqlx::query("DELETE FROM machines WHERE user_id = $1").bind(uid).execute(&pool).await.ok();
        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
    }

    /// DB-gated: a crashed or killed child frees its quota slot, while a
    /// completed-successful child and a still-running one both keep counting.
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
        for id in [&parent_id, &running, &completed, &crashed, &killed] {
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

        assert_eq!(
            live_child_count(&pool, &parent_id).await,
            2,
            "running + completed count; crashed + killed are freed"
        );

        sqlx::query("DELETE FROM sessions WHERE user_id = $1").bind(uid).execute(&pool).await.ok();
        sqlx::query("DELETE FROM machines WHERE user_id = $1").bind(uid).execute(&pool).await.ok();
        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
    }

    /// The ceiling a child is handed is never larger than the budget it was
    /// itself granted, so a spawn tree cannot outgrow its root.
    #[test]
    fn an_inherited_ceiling_only_shrinks_down_the_tree() {
        let root = cap(&["claude-code"], Some(20.0), Some(3));
        let child = root.inherited("root", Some(5.0), None);
        assert_eq!(child.max_budget_usd, Some(5.0));
        assert_eq!(child.adapters, root.adapters);
        assert_eq!(child.max_children, Some(3));

        let grandchild = child.inherited("root", Some(50.0), None);
        assert_eq!(
            grandchild.max_budget_usd,
            Some(5.0),
            "a child cannot hand a descendant more than its own ceiling"
        );

        assert_eq!(root.inherited("root", None, None).max_budget_usd, Some(20.0));
        assert_eq!(
            cap(&["codex"], None, None).inherited("root", Some(2.0), None).max_budget_usd,
            Some(2.0)
        );
        assert_eq!(cap(&["codex"], None, None).inherited("root", None, None).max_budget_usd, None);
    }

    /// A child spawned under the default grant inherits the budget it was
    /// actually given, not the unclamped default.
    #[test]
    fn a_child_of_the_default_grant_inherits_its_own_budget() {
        let root = SpawnCapability::machine_default();
        let granted = authorize(Some(&root), &req("claude-code", Some(1.5)), &usage(0)).unwrap();
        let child = root.inherited("root", granted.budget_usd, None);
        assert_eq!(child.max_budget_usd, Some(1.5));
        assert!(
            authorize(Some(&child), &req("claude-code", Some(2.0)), &usage(0)).is_err(),
            "the grandchild request must not exceed the child's inherited ceiling"
        );
        assert_eq!(
            authorize(Some(&child), &req("claude-code", None), &usage(0)).unwrap().budget_usd,
            Some(1.5)
        );
    }

    #[test]
    fn empty_prompt_or_adapter_is_a_bad_request() {
        let cap = cap(&["opencode"], Some(1.0), None);
        let mut blank_prompt = req("opencode", None);
        blank_prompt.prompt = "  ".into();
        assert!(matches!(
            authorize(Some(&cap), &blank_prompt, &usage(0)),
            Err(Denied::BadRequest(_))
        ));
        assert!(matches!(
            authorize(Some(&cap), &req("  ", None), &usage(0)),
            Err(Denied::BadRequest(_))
        ));
    }

    #[test]
    fn a_parent_in_ask_mode_cannot_mint_a_yolo_child() {
        let cap = cap(&["claude-code"], Some(1.0), None);
        let mut r = req("claude-code", None);
        r.permission_mode = Some(PermissionMode::Yolo);
        let ask = Usage { parent_mode: Some(PermissionMode::Ask), ..Usage::default() };
        assert_eq!(
            authorize(Some(&cap), &r, &ask),
            Err(Denied::PermissionMode {
                requested: PermissionMode::Yolo,
                max: PermissionMode::Ask
            })
        );
        r.permission_mode = Some(PermissionMode::Whip);
        assert!(matches!(authorize(Some(&cap), &r, &ask), Err(Denied::PermissionMode { .. })));

        let ceiling =
            SpawnCapability { max_permission_mode: Some(PermissionMode::Auto), ..cap.clone() };
        r.permission_mode = Some(PermissionMode::Yolo);
        let yolo = Usage { parent_mode: Some(PermissionMode::Yolo), ..Usage::default() };
        assert!(
            matches!(authorize(Some(&ceiling), &r, &yolo), Err(Denied::PermissionMode { .. })),
            "the capability ceiling holds even when the parent was toggled to yolo"
        );
        r.permission_mode = Some(PermissionMode::Ask);
        assert_eq!(
            authorize(Some(&ceiling), &r, &yolo).unwrap().permission_mode,
            PermissionMode::Ask
        );
    }

    #[test]
    fn a_child_naming_no_mode_inherits_the_parents() {
        let cap = cap(&["claude-code"], Some(1.0), None);
        for mode in [PermissionMode::Ask, PermissionMode::Auto, PermissionMode::Yolo] {
            let u = Usage { parent_mode: Some(mode), ..Usage::default() };
            assert_eq!(
                authorize(Some(&cap), &req("claude-code", None), &u).unwrap().permission_mode,
                mode
            );
        }
        let stored = SpawnCapability { max_permission_mode: Some(PermissionMode::Auto), ..cap };
        assert_eq!(
            authorize(Some(&stored), &req("claude-code", None), &usage(0)).unwrap().permission_mode,
            PermissionMode::Auto,
            "with no live mode the stored ceiling is the parent's posture"
        );
        let child = stored.inherited("root", Some(1.0), Some(PermissionMode::Ask));
        assert_eq!(child.max_permission_mode, Some(PermissionMode::Ask));
    }

    #[test]
    fn session_labels_parse_to_postures() {
        assert_eq!(PermissionMode::from_session_label("default"), Some(PermissionMode::Ask));
        assert_eq!(PermissionMode::from_session_label("plan"), Some(PermissionMode::Ask));
        assert_eq!(PermissionMode::from_session_label("acceptEdits"), Some(PermissionMode::Auto));
        assert_eq!(
            PermissionMode::from_session_label("bypassPermissions"),
            Some(PermissionMode::Yolo)
        );
        assert_eq!(PermissionMode::from_session_label("yolo"), Some(PermissionMode::Yolo));
        assert_eq!(PermissionMode::from_session_label("nonsense"), None);
    }

    #[test]
    fn spawning_past_max_depth_is_denied() {
        let root = SpawnCapability { max_depth: Some(2), ..cap(&["codex"], Some(1.0), None) };
        assert!(authorize(Some(&root), &req("codex", None), &usage(0)).is_ok());
        let child = root.inherited("root", Some(1.0), None);
        assert_eq!(child.max_depth, Some(1));
        assert!(authorize(Some(&child), &req("codex", None), &usage(0)).is_ok());
        let grandchild = child.inherited("child", Some(1.0), None);
        assert_eq!(grandchild.max_depth, Some(0));
        assert_eq!(
            authorize(Some(&grandchild), &req("codex", None), &usage(0)),
            Err(Denied::Depth)
        );
        assert_eq!(grandchild.tree_root.as_deref(), Some("root"), "the root id carries down");
    }

    #[test]
    fn machine_default_is_finite() {
        let root = SpawnCapability::machine_default();
        assert_eq!(root.max_depth, Some(cctui_proto::api::DEFAULT_MAX_DEPTH));
        assert_eq!(root.max_children, Some(cctui_proto::api::DEFAULT_MAX_CHILDREN));
        assert!(root.max_tree_budget_usd.is_some_and(f64::is_finite));
        assert!(cctui_proto::api::DEFAULT_MAX_DEPTH >= 2);
    }

    #[test]
    fn with_no_mode_information_the_ceiling_is_ask() {
        let root = SpawnCapability::machine_default();
        let mut r = req("claude-code", None);
        assert_eq!(
            authorize(Some(&root), &r, &usage(0)).unwrap().permission_mode,
            PermissionMode::Ask
        );
        r.permission_mode = Some(PermissionMode::Yolo);
        assert!(matches!(
            authorize(Some(&root), &r, &usage(0)),
            Err(Denied::PermissionMode { max: PermissionMode::Ask, .. })
        ));
    }

    /// A session launched in yolo is stamped with a yolo ceiling at spawn time,
    /// and its tree keeps spawning yolo descendants even when no session row
    /// ever reports a live mode.
    #[test]
    fn a_yolo_launched_tree_spawns_yolo_children_and_grandchildren() {
        let root = SpawnCapability {
            max_permission_mode: Some(PermissionMode::Yolo),
            ..SpawnCapability::machine_default()
        };
        let unknown = usage(0);
        let mut explicit = req("claude-code", None);
        explicit.permission_mode = Some(PermissionMode::Yolo);
        let implicit = req("codex", None);

        let mut node = root;
        let mut id = "root".to_owned();
        for generation in 1..=cctui_proto::api::DEFAULT_MAX_DEPTH {
            let a = authorize(Some(&node), &explicit, &unknown).unwrap();
            assert_eq!(a.permission_mode, PermissionMode::Yolo, "generation {generation}");
            let b = authorize(Some(&node), &implicit, &unknown).unwrap();
            assert_eq!(b.permission_mode, PermissionMode::Yolo, "an omitted mode inherits yolo");
            node = node.inherited(&id, a.budget_usd, Some(a.permission_mode));
            assert_eq!(node.max_permission_mode, Some(PermissionMode::Yolo));
            id = format!("gen-{generation}");
        }
        assert_eq!(authorize(Some(&node), &explicit, &unknown), Err(Denied::Depth));

        let reported_yolo = Usage { parent_mode: Some(PermissionMode::Yolo), ..Usage::default() };
        let child =
            SpawnCapability::machine_default().inherited("r", None, Some(PermissionMode::Yolo));
        assert_eq!(
            authorize(Some(&child), &explicit, &reported_yolo).unwrap().permission_mode,
            PermissionMode::Yolo
        );
        let toggled_to_ask = Usage { parent_mode: Some(PermissionMode::Ask), ..Usage::default() };
        assert!(
            authorize(Some(&child), &explicit, &toggled_to_ask).is_err(),
            "a yolo tree whose parent was switched to ask stops minting yolo children"
        );
    }

    #[test]
    fn granted_budgets_under_one_root_never_exceed_the_tree_ceiling() {
        let root =
            SpawnCapability { max_tree_budget_usd: Some(10.0), ..cap(&["codex"], Some(4.0), None) };
        let mut granted = 0.0;
        let mut denied = false;
        for i in 0..10 {
            let parent =
                if i % 2 == 0 { root.clone() } else { root.inherited("root", Some(4.0), None) };
            let u = Usage { tree_granted_usd: granted, ..Usage::default() };
            match authorize(Some(&parent), &req("codex", None), &u) {
                Ok(a) => granted += a.budget_usd.unwrap(),
                Err(Denied::TreeBudget { .. }) => denied = true,
                Err(e) => panic!("unexpected denial {e}"),
            }
            assert!(granted <= 10.0, "granted {granted} past the tree ceiling");
        }
        assert!(denied);
        assert!(
            (granted - 10.0).abs() < f64::EPSILON,
            "the last grant is clamped to the remainder"
        );

        let u = Usage { tree_granted_usd: 8.0, ..Usage::default() };
        assert_eq!(
            authorize(Some(&root), &req("codex", Some(3.0)), &u),
            Err(Denied::TreeBudget { requested: 3.0, remaining: 2.0 })
        );
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
        assert_eq!(tree_granted_usd(&pool, &root_id).await.unwrap(), 5.0);

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
        assert_eq!(err.0, StatusCode::FORBIDDEN, "the tree is spent");

        release_child(&pool, &grandchild_key).await;
        assert_eq!(tree_granted_usd(&pool, &root_id).await.unwrap(), 3.0);

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
        assert_eq!(err.0, StatusCode::FORBIDDEN);
        assert!(err.1.error.contains("maximum of 2"), "{}", err.1.error);

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
}
