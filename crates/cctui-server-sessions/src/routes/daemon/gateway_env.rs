use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use uuid::Uuid;

use crate::context::SessionsContext;
use cctui_proto::features::Feature;
use cctui_telemetry::db::traced;
use tracing::Instrument;

// ---- /api/v1/daemon/sessions/{id}/gateway-env ----

/// Query of both launch-context pulls. `rules` is absent from daemons that
/// predate rules.
#[derive(Debug, Default, Deserialize)]
pub struct LaunchQuery {
    #[serde(default)]
    rules: bool,
}

/// Resolve a session's gateway-routing env for the daemon's launch chokepoint.
///
/// The daemon calls this at every worker (re)launch — spawn, resume,
/// cold-resume, fork — so the gateway credential comes from the server's durable
/// `sessions.account_id` binding rather than from whatever env the triggering
/// command happened to carry. This is what makes routing survive a daemon /
/// claude-daemon restart and session-id rotation: the env is re-derived from the
/// DB, not from volatile process/in-memory state.
///
/// Self-authenticating like [`auth`]/[`ws`]: the machine key is the Bearer.
/// Scoped to the machine's owning user so a daemon can't resolve another user's
/// account env.
///
/// Returns `{account_bound, env}`:
///   * no binding → `{false, {}}` (no gateway routing needed; launch as-is)
///   * bound + mintable → `{true, env}` (inject and launch)
///   * bound + unmintable (account gone) → `{true, {}}` (daemon fails closed)
///   * transient DB error → 500 (daemon falls back to the pushed env hint)
pub async fn session_gateway_env<S: SessionsContext>(
    State(state): State<S>,
    headers: axum::http::HeaderMap,
    Path(session_id): Path<String>,
    Query(query): Query<LaunchQuery>,
) -> Result<Json<cctui_proto::api::GatewayEnvResponse>, StatusCode> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let ctx = state.auth_config().validate(token).await.ok_or(StatusCode::UNAUTHORIZED)?;
    let Some(machine_id) = ctx.machine_id else {
        return Err(StatusCode::FORBIDDEN);
    };
    gateway_env_for(&state, ctx.user_id, machine_id, &session_id, query.rules).await.map(Json)
}

/// Every branch runs to completion, never cancelled by a sibling's error: a
/// mint dropped between its token write and its `sessions` write would leave
/// the binding half-moved.
#[tracing::instrument(
    target = "otel",
    skip_all,
    fields(code.function.name = "gateway_env_for", session_id = %session_id),
)]
pub async fn gateway_env_for<S: SessionsContext>(
    state: &S,
    user_id: Uuid,
    machine_id: Uuid,
    session_id: &str,
    rules: bool,
) -> Result<cctui_proto::api::GatewayEnvResponse, StatusCode> {
    let allowed =
        state.gateway_env_allowed(user_id, machine_id, session_id).await.map_err(|e| {
            tracing::error!(%session_id, "daemon gateway-env ownership lookup failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    if !allowed {
        return Ok(cctui_proto::api::GatewayEnvResponse {
            account_bound: false,
            env: std::collections::BTreeMap::default(),
            settings: None,
            whip_phrases: None,
            spawn_capability: None,
            plugins: Vec::new(),
            context: Vec::new(),
            hook_policy: None,
        });
    }

    let with_plugins = state.config().features.contains(Feature::Plugins);
    let (user_settings, (), spawn_capability, context, bound, hook_policy) = tokio::join!(
        user_settings_of(state, user_id),
        async {
            if with_plugins {
                sync_plugins(state).await;
            }
        },
        state.spawn_capability_for(session_id),
        owned_launch_context(state, user_id, session_id, rules),
        bound_launch(state, session_id),
        recorded_hook_policy(state, session_id),
    );
    let context = context?;
    let bound = bound?;
    let hook_policy = hook_policy?;
    // The machine user's whip stall-phrase override rides this pull to reach
    // the connectionless `whip-stop-hook`, bound or not.
    let whip_phrases =
        user_settings.as_ref().and_then(crate::routes::settings::whip_stop_phrases_of);
    let plugins =
        if with_plugins { state.session_plugins(user_settings.as_ref()) } else { Vec::new() };
    let (account_bound, env, settings) = bound.map_or_else(
        || (false, std::collections::BTreeMap::default(), None),
        |BoundLaunch { env, settings }| (true, env, settings),
    );
    Ok(cctui_proto::api::GatewayEnvResponse {
        account_bound,
        env,
        settings,
        whip_phrases,
        spawn_capability,
        plugins,
        context,
        hook_policy: Some(hook_policy),
    })
}

#[tracing::instrument(
    target = "otel",
    skip_all,
    fields(code.function.name = "recorded_hook_policy", session_id = %session_id),
)]
async fn recorded_hook_policy<S: SessionsContext>(
    state: &S,
    session_id: &str,
) -> Result<cctui_proto::hook_policy::SessionHookPolicy, StatusCode> {
    state.hook_policy_for_session(session_id).await.map_err(|e| {
        tracing::error!(%session_id, "daemon gateway-env hook policy lookup failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })
}

#[tracing::instrument(target = "otel", skip_all, fields(code.function.name = "sync_plugins"))]
async fn sync_plugins<S: SessionsContext>(state: &S) {
    state.sync_or_warn().await;
}

/// Re-mint every bound family (one account each, disjoint env keys) and serve
/// their merged `settings_json`; `None` when the session binds no account.
///
/// A family whose account row is gone is skipped, so with every family gone
/// the env is empty and the daemon fails closed. A DB failure is a 500: the
/// daemon then falls back to its pushed env hint.
#[tracing::instrument(
    target = "otel",
    skip_all,
    fields(code.function.name = "bound_launch", session_id = %session_id),
)]
async fn bound_launch<S: SessionsContext>(
    state: &S,
    session_id: &str,
) -> Result<Option<BoundLaunch>, StatusCode> {
    let binding = state.resolve_session_binding(session_id).await;
    if binding.accounts.is_empty() {
        return Ok(None);
    }
    let (env, mut settings, adapter) = tokio::join!(
        state.mint_binding_env(session_id, &binding),
        state.binding_settings(session_id, &binding),
        session_adapter(state, session_id),
    );
    let env = env.map_err(|e| {
        tracing::error!(%session_id, "daemon gateway-env mint failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    // Codex defaults to the `priority` tier, so a relaunch with no opinion is the
    // expensive one. Serve a concrete tier to a codex session, and only to one:
    // the OpenAI-wire ACP agents share the family but cannot use the key.
    if cctui_server_kernel::execution_settings::adapter_takes_service_tier(&adapter) {
        settings = cctui_server_kernel::execution_settings::overlay_service_tier(settings);
    }
    // This pull only happens when the daemon is actually (re)launching the
    // worker, so a session marked `ended` (possibly by a spurious end) is
    // coming back to life. `archived` stays parked: un-archiving is an explicit
    // user action.
    let _ = traced(
        "UPDATE sessions SET status = 'active', ended_at = NULL, end_reason = NULL, \
         end_detail = NULL WHERE id = $1 AND status = 'ended'",
        |sql| sqlx::query(sql).bind(session_id).execute(state.pool()),
    )
    .await;
    Ok(Some(BoundLaunch { env, settings }))
}

struct BoundLaunch {
    env: std::collections::BTreeMap<String, String>,
    settings: Option<serde_json::Value>,
}

async fn session_adapter<S: SessionsContext>(state: &S, session_id: &str) -> String {
    crate::store::sessions::adapter_id(state.pool(), session_id)
        .instrument(tracing::info_span!(
            target: "otel",
            "session_adapter",
            code.function.name = "session_adapter"
        ))
        .await
        .ok()
        .flatten()
        .unwrap_or_default()
}

/// `GET /api/v1/daemon/sessions/{id}/context` — [`session_context`] without
/// minting credentials, for a launch that already holds them.
pub async fn session_launch_context<S: SessionsContext>(
    State(state): State<S>,
    headers: axum::http::HeaderMap,
    Path(session_id): Path<String>,
    Query(query): Query<LaunchQuery>,
) -> Result<Json<cctui_proto::api::LaunchContextResponse>, StatusCode> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let ctx = state.auth_config().validate(token).await.ok_or(StatusCode::UNAUTHORIZED)?;
    let Some(machine_id) = ctx.machine_id else {
        return Err(StatusCode::FORBIDDEN);
    };
    let context = launch_context(&state, ctx.user_id, machine_id, &session_id, query.rules).await?;
    Ok(Json(cctui_proto::api::LaunchContextResponse { context }))
}

/// [`session_context`] gated by [`owns_launch`]: both launch pulls serve the
/// context only when the key is the caller's, and only with `context` on.
pub async fn launch_context<S: SessionsContext>(
    state: &S,
    user_id: Uuid,
    machine_id: Uuid,
    session_id: &str,
    rules: bool,
) -> Result<Vec<cctui_proto::api::SessionContextItem>, StatusCode> {
    let allowed = owns_launch(state, user_id, machine_id, session_id).await.map_err(|e| {
        tracing::error!(%session_id, "daemon launch ownership lookup failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(if allowed && state.config().features.contains(Feature::Context) {
        session_context(state.pool(), session_id, rules).await
    } else {
        Vec::new()
    })
}

/// [`launch_context`] for a caller [`gateway_env_allowed`] already let through.
#[tracing::instrument(
    target = "otel",
    skip_all,
    fields(code.function.name = "owned_launch_context", session_id = %session_id),
)]
async fn owned_launch_context<S: SessionsContext>(
    state: &S,
    user_id: Uuid,
    session_id: &str,
    rules: bool,
) -> Result<Vec<cctui_proto::api::SessionContextItem>, StatusCode> {
    let owned = owns_launch_key(state.pool(), user_id, session_id).await.map_err(|e| {
        tracing::error!(%session_id, "daemon launch ownership lookup failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(if owned && state.config().features.contains(Feature::Context) {
        session_context(state.pool(), session_id, rules).await
    } else {
        Vec::new()
    })
}

pub async fn owns_launch<S: SessionsContext>(
    state: &S,
    user_id: Uuid,
    machine_id: Uuid,
    session_id: &str,
) -> Result<bool, sqlx::Error> {
    let pool = state.pool();
    if !state.gateway_env_allowed(user_id, machine_id, session_id).await? {
        return Ok(false);
    }
    owns_launch_key(pool, user_id, session_id).await
}

/// A key with no session row is the caller's only when its ownership record
/// names the caller.
async fn owns_launch_key(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    session_id: &str,
) -> Result<bool, sqlx::Error> {
    let registered: bool = traced("SELECT EXISTS (SELECT 1 FROM sessions WHERE id = $1)", |sql| {
        sqlx::query_scalar(sql).bind(session_id).fetch_one(pool)
    })
    .await?;
    if registered {
        return Ok(true);
    }
    let owner = crate::routes::context::ownership::owner_of(pool, session_id).await?;
    Ok(owner == Some(user_id))
}

/// The context this launch carries, as the daemon needs it: the rules,
/// re-resolved now so an edited rule reaches the next launch, then the memory
/// notes and prompt template the spawn froze. Read through the spawn key the
/// adapters echo.
async fn session_context(
    pool: &sqlx::PgPool,
    session_id: &str,
    rules: bool,
) -> Vec<cctui_proto::api::SessionContextItem> {
    crate::routes::context::for_launch(pool, session_id, rules)
        .instrument(tracing::info_span!(
            target: "otel",
            "session_context",
            code.function.name = "session_context"
        ))
        .await
        .into_iter()
        .map(|i| cctui_proto::api::SessionContextItem {
            kind: i.kind,
            name: i.name,
            title: i.title,
            body: i.body,
            version: i.version,
        })
        .collect()
}

/// The machine user's clamped `whipStopPhrases` block from
/// `user_settings.data`, or `None` when unset / reduced to the default.
///
/// Read from
/// the DB on the same gateway-env pull that carries the account settings.
pub async fn user_settings_of<S: SessionsContext>(
    state: &S,
    user_id: Uuid,
) -> Option<serde_json::Value> {
    traced("SELECT data FROM user_settings WHERE user_id = $1", |sql| {
        sqlx::query_scalar(sql).bind(user_id).fetch_optional(state.pool())
    })
    .await
    .ok()
    .flatten()
}

// ---- /api/v1/daemon/sessions/{id}/token-valid ----

/// Query for [`session_token_valid`]: the sha256 hex of the session token the
/// daemon launched the worker with. Hash-only on purpose — no token material
/// on the wire (invariant).
#[derive(Deserialize)]
pub struct TokenValidQuery {
    pub hash: String,
}

/// Does this session's minted token still resolve at the gateway?
///
/// The daemon's validity sweep calls this for TRUSTED workers (ones it
/// launched with gateway env) so a worker whose `session_tokens` row got
/// unbound/deleted (401s forever at the gateway) is observable and healable.
/// `valid` = a `session_tokens` row with this hash exists
/// FOR THIS SESSION, is not revoked, and joins a live `account_providers` row
/// (the same join [`resolve_account`](crate::routes::gateway) applies, but by
/// hash equality).
///
/// Self-authenticating like [`session_gateway_env`]: machine-key Bearer.
/// User-scoped the same way, except a session owned by another user answers
/// 404 rather than `{valid: false}` — a false `valid` triggers a destructive
/// kill + cold-resume daemon-side, and the daemon treats any non-200 as
/// "unknown" (no heal). Transient DB error → 500 for the same reason
/// (fail-open; the heal kill is destructive).
pub async fn session_token_valid<S: SessionsContext>(
    State(state): State<S>,
    headers: axum::http::HeaderMap,
    Path(session_id): Path<String>,
    axum::extract::Query(q): axum::extract::Query<TokenValidQuery>,
) -> Result<Json<cctui_proto::api::TokenValidResponse>, StatusCode> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let ctx = state.auth_config().validate(token).await.ok_or(StatusCode::UNAUTHORIZED)?;
    if ctx.machine_id.is_none() {
        return Err(StatusCode::FORBIDDEN);
    }

    let valid = state
        .token_valid(ctx.user_id, &session_id, &q.hash)
        .await
        .map_err(|error| {
            tracing::error!(%session_id,%error,"token-valid lookup failed");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(cctui_proto::api::TokenValidResponse { valid }))
}
