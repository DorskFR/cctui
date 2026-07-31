//! `POST /api/v1/daemon/sessions/{id}/spawn-child` — the server side of the
//! daemon's `CctuiAgent` tool.
//!
//! A session asks its daemon to spawn a subagent; the daemon relays the request
//! here with its machine key. The server is the only place the decision is made:
//! it reads the calling session's [`SpawnCapability`] — set by whoever launched
//! that session, never writable by the session itself — and refuses anything the
//! capability does not name. No capability at all ⇒ deny.
//!
//! An authorized child goes down the ordinary spawn path (pre-minted session id,
//! account-bound gateway env, `AdapterCommand::Spawn` over the daemon WS), with
//! `parent_local_id` set so it registers as a real, nested, meterable session.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;

use cctui_proto::adapter::{AdapterCommand, AdapterId, SessionSpec};
use cctui_proto::api::{ApiError, SpawnCapability, SpawnChildRequest, SpawnChildResponse};
use cctui_proto::ws::DaemonFrameDown;
use uuid::Uuid;

/// The parent session's launch context, everything a child inherits.
struct Parent {
    session_id: String,
    machine_uuid: Uuid,
    working_dir: Option<String>,
    user_id: Uuid,
}

/// A spawn request that cleared the capability check.
#[derive(Debug, PartialEq)]
pub struct Authorized {
    pub adapter: String,
    pub budget_usd: Option<f64>,
}

/// Why a `CctuiAgent` call was refused. Rendered verbatim into the tool result,
/// so each variant names the limit that stopped it.
#[derive(Debug, PartialEq)]
pub enum Denied {
    NoCapability,
    Adapter { requested: String, allowed: Vec<String> },
    Budget { requested: f64, max: Option<f64> },
    TooManyChildren { max: u32 },
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
            Self::BadRequest(msg) => f.write_str(msg),
        }
    }
}

/// Decide whether `req` is permitted by `cap`, and with what budget.
///
/// Pure and fail-closed: an absent capability, an unlisted adapter, a budget
/// over the ceiling, or a child count at the cap all deny. A call that names no
/// budget inherits the capability's ceiling, so a child is never unbudgeted when
/// the parent is budgeted.
pub fn authorize(
    cap: Option<&SpawnCapability>,
    req: &SpawnChildRequest,
    live_children: u32,
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
    if let Some(max) = cap.max_children
        && live_children >= max
    {
        return Err(Denied::TooManyChildren { max });
    }
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
    Ok(Authorized { adapter: adapter.to_owned(), budget_usd: budget })
}

use crate::state::AppState;

fn deny(code: StatusCode, msg: impl Into<String>) -> (StatusCode, Json<ApiError>) {
    (code, Json(ApiError { error: msg.into() }))
}

/// Authenticate the caller as a daemon machine key and return its user id.
async fn machine_user(
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
    let row: Option<(Option<Uuid>, Option<String>, Option<Uuid>)> =
        sqlx::query_as("SELECT machine_uuid, working_dir, user_id FROM sessions WHERE id = $1")
            .bind(session_id)
            .fetch_optional(&state.pool)
            .await
            .map_err(|e| {
                tracing::error!(%session_id, "db error (spawn-child parent): {e}");
                deny(StatusCode::INTERNAL_SERVER_ERROR, "database error")
            })?;
    let Some((machine_uuid, working_dir, user_id)) = row else {
        return Err(deny(StatusCode::NOT_FOUND, "calling session not found"));
    };
    let (Some(machine_uuid), Some(user_id)) = (machine_uuid, user_id) else {
        return Err(deny(StatusCode::CONFLICT, "calling session has no machine/owner"));
    };
    if user_id != caller {
        return Err(deny(StatusCode::FORBIDDEN, "session belongs to another user"));
    }
    Ok(Parent { session_id: session_id.to_owned(), machine_uuid, working_dir, user_id })
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

async fn child_count(state: &AppState, parent_id: &str) -> u32 {
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions WHERE parent_id = $1")
        .bind(parent_id)
        .fetch_one(&state.pool)
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
    let authorized = authorize(cap.as_ref(), &req, child_count(&state, &session_id).await)
        .map_err(|d| match d {
            Denied::BadRequest(_) => deny(StatusCode::BAD_REQUEST, d.to_string()),
            _ => deny(StatusCode::FORBIDDEN, d.to_string()),
        })?;

    let child_id = Uuid::new_v4();
    let child_key = child_id.to_string();
    let family = crate::routes::gateway::Family::from_adapter(&authorized.adapter);
    let (mut env, model) =
        child_account_env(&state, &parent, family, req.model.as_deref(), &child_key).await?;
    if let Some(profile) = req.agent_profile.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        env.insert(AGENT_PROFILE_ENV.to_owned(), profile.to_owned());
    }

    let spec = SessionSpec {
        adapter_id: AdapterId::new(&authorized.adapter),
        working_dir: req
            .cwd
            .clone()
            .filter(|c| !c.trim().is_empty())
            .or_else(|| parent.working_dir.clone()),
        prompt: Some(req.prompt.clone()),
        name: None,
        permission_mode: None,
        effort: None,
        model,
        env,
        bootstrap: serde_json::Value::Null,
        parent_local_id: Some(parent.session_id.clone()),
    };
    let frame = DaemonFrameDown::Command {
        adapter_id: authorized.adapter.clone(),
        command: Box::new(AdapterCommand::Spawn {
            spec,
            command_id: Some(child_id),
            session_id: Some(child_id),
        }),
    };
    state.bus.command_daemon(parent.machine_uuid, frame).await.map_err(|err| {
        deny(StatusCode::SERVICE_UNAVAILABLE, format!("could not reach the daemon: {err}"))
    })?;

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

/// Env key the opencode adapter reads to select an agent profile.
pub const AGENT_PROFILE_ENV: &str = "CCTUI_OPENCODE_AGENT";

#[cfg(test)]
mod tests {
    use super::*;

    fn cap(
        adapters: &[&str],
        max_budget: Option<f64>,
        max_children: Option<u32>,
    ) -> SpawnCapability {
        SpawnCapability {
            adapters: adapters.iter().map(|s| (*s).to_owned()).collect(),
            max_budget_usd: max_budget,
            max_children,
        }
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
        assert_eq!(authorize(None, &req("opencode", None), 0), Err(Denied::NoCapability));
    }

    #[test]
    fn empty_adapter_list_denies_like_no_capability() {
        let cap = cap(&[], Some(1.0), None);
        assert_eq!(authorize(Some(&cap), &req("opencode", None), 0), Err(Denied::NoCapability));
    }

    #[test]
    fn unlisted_adapter_denies_and_names_the_allowed_set() {
        let cap = cap(&["opencode"], Some(1.0), None);
        let err = authorize(Some(&cap), &req("claude-code", None), 0).unwrap_err();
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
        let ok = authorize(Some(&cap), &req("opencode", None), 0).unwrap();
        assert_eq!(ok, Authorized { adapter: "opencode".into(), budget_usd: Some(2.5) });
    }

    #[test]
    fn budget_over_the_ceiling_denies() {
        let cap = cap(&["opencode"], Some(2.0), None);
        assert_eq!(
            authorize(Some(&cap), &req("opencode", Some(5.0)), 0),
            Err(Denied::Budget { requested: 5.0, max: Some(2.0) })
        );
    }

    #[test]
    fn budget_requested_without_a_ceiling_denies() {
        let cap = cap(&["opencode"], None, None);
        assert_eq!(
            authorize(Some(&cap), &req("opencode", Some(0.5)), 0),
            Err(Denied::Budget { requested: 0.5, max: None })
        );
    }

    #[test]
    fn budget_at_the_ceiling_is_allowed() {
        let cap = cap(&["opencode"], Some(2.0), None);
        assert_eq!(
            authorize(Some(&cap), &req("opencode", Some(2.0)), 0).unwrap().budget_usd,
            Some(2.0)
        );
    }

    #[test]
    fn nonpositive_or_nonfinite_budget_is_a_bad_request() {
        let cap = cap(&["opencode"], Some(2.0), None);
        assert!(matches!(
            authorize(Some(&cap), &req("opencode", Some(0.0)), 0),
            Err(Denied::BadRequest(_))
        ));
        assert!(matches!(
            authorize(Some(&cap), &req("opencode", Some(f64::NAN)), 0),
            Err(Denied::BadRequest(_))
        ));
    }

    #[test]
    fn child_cap_denies_once_reached() {
        let cap = cap(&["opencode"], Some(1.0), Some(2));
        assert!(authorize(Some(&cap), &req("opencode", None), 1).is_ok());
        assert_eq!(
            authorize(Some(&cap), &req("opencode", None), 2),
            Err(Denied::TooManyChildren { max: 2 })
        );
    }

    #[test]
    fn empty_prompt_or_adapter_is_a_bad_request() {
        let cap = cap(&["opencode"], Some(1.0), None);
        let mut blank_prompt = req("opencode", None);
        blank_prompt.prompt = "  ".into();
        assert!(matches!(authorize(Some(&cap), &blank_prompt, 0), Err(Denied::BadRequest(_))));
        assert!(matches!(authorize(Some(&cap), &req("  ", None), 0), Err(Denied::BadRequest(_))));
    }
}
