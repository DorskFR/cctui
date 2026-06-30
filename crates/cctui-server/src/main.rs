mod archive_store;
mod auth;
mod authz;
mod config;
mod crypto;
mod daemon_dispatch;
mod db;
mod dispatchers;
mod langfuse;
mod machine_liveness;
mod normalize;
mod ntfy;
mod policy;
mod rebuild;
mod registry;
mod routes;
mod skill_store;
mod soft_limit;
mod state;
mod uploads;
mod webhook;
mod ws;

use std::path::PathBuf;
use std::sync::Arc;

use authz::{Action, Authn, Authz, IdFrom, ResourceKind, Routes};
use axum::extract::DefaultBodyLimit;
use axum::http::Method;
use axum::routing::{any, delete, get, patch, post, put};
use axum::{Extension, Router, middleware};
use config::Config;
use registry::Registry;
use state::AppState;

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "cctui_server=info".into()),
        )
        .init();

    let config = Config::from_env();
    let pool = db::connect(&config.database_url).await?;
    // One-release back-compat shim (CCT-399): if the retired
    // CCTUI_CLAUDE_LITELLM_* env vars are set, synthesize a managed (read-only)
    // anthropic-compatible account per user so existing deployments keep working
    // until they migrate to first-class accounts.
    routes::accounts::sync_litellm_shim(&pool, &config).await;
    let auth_config = auth::AuthConfig::new(Config::admin_tokens(), pool.clone());
    // CCT-410: resolve CCTUI_ADMIN_TOKENS to a seeded admin user + api_keys rows
    // with {admin} ceiling/grant, so the break-glass token is a real identity
    // rather than a user_id=None ghost. Idempotent, best-effort.
    auth_config.seed_admin().await;
    let (tui_tx, _) = tokio::sync::broadcast::channel(256);

    let archive = init_archive_store().await;
    let skills = init_skill_store().await;
    let dispatchers = init_dispatchers(&config).await;

    let state = AppState {
        pool,
        config: config.clone(),
        registry: Registry::shared(),
        permission_store: routes::permissions::PermissionStore::shared(),
        tui_tx,
        auth_config: auth_config.clone(),
        archive,
        skills,
        daemon_connections: Arc::new(dashmap::DashMap::new()),
        dispatcher_connections: Arc::new(dashmap::DashMap::new()),
        pending_dispatcher_requests: Arc::new(dashmap::DashMap::new()),
        dispatcher_liveness: Arc::new(dashmap::DashMap::new()),
        dispatchers,
        pending_stage_requests: Arc::new(dashmap::DashMap::new()),
        pending_listdirs_requests: Arc::new(dashmap::DashMap::new()),
        machine_liveness: Arc::new(dashmap::DashMap::new()),
        account_locks: Arc::new(dashmap::DashMap::new()),
        http_client: reqwest::Client::new(),
        // Optional Langfuse tracing sink (CCT-443). `None` (dark) unless the
        // CCTUI_LANGFUSE_* env is fully set — zero overhead on the gateway path.
        langfuse: langfuse::LangfuseConfig::from_env()
            .map(|c| Arc::new(langfuse::LangfuseClient::new(c, reqwest::Client::new()))),
        pending_oauth_logins: Arc::new(dashmap::DashMap::new()),
        account_usage_cache: Arc::new(dashmap::DashMap::new()),
        pr_status_cache: cctui_proto::classifier::PrStatusCache::new(),
        soft_limit_blocked: Arc::new(dashmap::DashMap::new()),
        gateway_orphan_spam: Arc::new(dashmap::DashMap::new()),
        account_reauth: Arc::new(dashmap::DashMap::new()),
    };

    // Warm the reauth gate from the persisted flag (CCT-512) so a restart doesn't
    // strand an account: without this the success path couldn't clear a flag set
    // before the restart (it only writes on the in-memory transition).
    if let Ok(ids) =
        sqlx::query_scalar::<_, uuid::Uuid>("SELECT id FROM oauth_accounts WHERE needs_reauth")
            .fetch_all(&state.pool)
            .await
    {
        for id in ids {
            state.account_reauth.insert(id, ());
        }
    }

    let (api_router, api_descriptors) = build_api_routes().into_parts();

    // The descriptor list is the route table / source of truth, consumed by
    // the coverage test. At runtime it is informational only.
    debug_assert!(!api_descriptors.is_empty());
    let _ = &api_descriptors;

    let api_router = api_router
        // Authentication runs as a global layer; AUTHORIZATION is enforced
        // per-route inside each route's `route_layer` (attached by
        // `Routes::add`), which runs INSIDE this `auth_middleware` so the
        // `AuthContext` it inserts is already present when the policy evaluates.
        // (A global authz layer would run OUTSIDE the matched route and could
        // not see its policy — the 0.7.0 default-deny regression.)
        .layer(middleware::from_fn(auth::auth_middleware))
        .layer(Extension(auth_config.clone()));

    // Optional GitHub integration (CCT-373 / GH-PKG-1). Behind the `github`
    // Cargo feature: run its embedded migrations and merge its routes. A build
    // without the feature contains zero GitHub code, routes, or schema.
    #[cfg(feature = "github")]
    cctui_github::migrate(&state.pool).await?;

    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/api/v1/ws", get(ws::tui_ws))
        // Browser auth-cookie endpoints (CCT-423). Self-authenticating: `login`
        // validates the presented token and sets the `HttpOnly` cookie, `logout`
        // clears it — both live outside the `auth_middleware` group.
        .route("/api/v1/auth/login", post(routes::auth::login))
        .route("/api/v1/auth/logout", post(routes::auth::logout))
        // Daemon-facing endpoints. `auth` and `ws` carry their own auth
        // (machine-key Bearer / `?token=` query) so they live outside the
        // user-token-only `api_router` group.
        .route("/api/v1/daemon/auth", post(routes::daemon::auth))
        .route("/api/v1/daemon/ws", get(routes::daemon::ws))
        // Launch-time gateway-env pull (CCT-460): the daemon resolves a
        // session's account env here on every worker (re)launch. Self-auths via
        // the machine-key Bearer, so it sits beside the other daemon endpoints.
        .route("/api/v1/daemon/sessions/{id}/gateway-env", get(routes::daemon::session_gateway_env))
        // Enrolled-dispatcher endpoints (CCT-285). Carry their own key auth
        // (dispatcher-key Bearer / `?token=`), so they live outside the
        // user-token `api_router` group, like the daemon endpoints.
        .route("/api/v1/dispatcher/auth", post(routes::dispatcher::auth))
        .route("/api/v1/dispatcher/ws", get(routes::dispatcher::ws))
        .route("/api/v1/triggers/{kind}", post(routes::triggers::ingest))
        // OAuth passthrough gateway (CCT-232). Auths via the session-scoped
        // token in the request's own Authorization header — NOT the user-token
        // `api_router` middleware — so it lives on the outer app. Matches any
        // method + sub-path under each provider prefix.
        .route("/gateway/anthropic/{*path}", any(routes::gateway::anthropic))
        .route("/gateway/openai/{*path}", any(routes::gateway::openai))
        .nest("/api/v1", api_router)
        // The web UI is served same-origin in prod, so the `HttpOnly` auth
        // cookie (CCT-423) flows without any cross-origin credential config.
        // The permissive policy remains safe for the API/daemon Bearer callers
        // (no credentialed cross-origin cookie reliance). WS upgrades read the
        // cookie on the same-origin upgrade and are not subject to CORS preflight.
        .layer(tower_http::cors::CorsLayer::permissive())
        .with_state(state.clone());

    // Merge the GitHub routes (their own state is already applied) under
    // `/api/v1`. Done after `with_state` because they carry `GithubState`, not
    // the server's `AppState`. The GitHub router lives outside `api_router`'s
    // layers, so we re-apply the same auth middleware here, plus a thin layer
    // that maps the server-private `AuthContext` into the proto `CallerIdentity`
    // the GitHub crate extracts (it cannot depend on `cctui-server`).
    #[cfg(feature = "github")]
    let app = app.nest(
        "/api/v1",
        cctui_github::routes(
            state.pool.clone(),
            state.tui_tx.clone(),
            state.pr_status_cache.clone(),
        )
        .layer(middleware::from_fn(github_identity))
        .layer(middleware::from_fn(auth::auth_middleware))
        .layer(Extension(auth_config.clone())),
    );

    // GH-AGENT-2: the agent MCP review endpoint authenticates the bearer session
    // token on its own (it is not a user/machine token `auth_middleware` knows),
    // so it is merged WITHOUT the auth/identity layers above.
    #[cfg(feature = "github")]
    let app = app.nest(
        "/api/v1",
        cctui_github::mcp_routes(
            state.pool.clone(),
            state.tui_tx.clone(),
            state.pr_status_cache.clone(),
        ),
    );

    // GH-CONN-4: the reconcile poll loop. A background task (mirroring
    // `reaper_task`) that heals missed webhooks and hydrates first install by
    // polling GitHub for "PRs involving me" per connector. Behind the `github`
    // feature; disabled when `CCTUI_GITHUB_RECONCILE_SECS=0`.
    #[cfg(feature = "github")]
    cctui_github::spawn_reconcile(
        state.pool.clone(),
        state.tui_tx.clone(),
        state.pr_status_cache.clone(),
    );

    tokio::spawn(reaper_task(state));

    let listener = tokio::net::TcpListener::bind(config.bind_addr()).await?;
    tracing::info!("listening on {}", config.bind_addr());
    axum::serve(listener, app).await?;
    Ok(())
}

/// Build the `/api/v1` route table from the descriptor list (CCT-419). Every
/// route declares both an [`Authn`] (recorded; the proven `auth_middleware`
/// path still performs authentication) and an [`Authz`] (enforced by
/// `authz::authz_layer`, default-deny for any un-policied route). Each route's
/// declared policy mirrors its CURRENT enforcement exactly: routes with
/// in-handler owner checks or `owner_filter()` SQL filters declare
/// `Authenticated` and keep that filter in the handler (the type system can't
/// express a self-scoped filter); scope-gated routes declare the matching
/// `Scope`. The returned [`Routes`] is the single source of truth walked by the
/// coverage test.
#[allow(clippy::too_many_lines)]
fn build_api_routes() -> Routes {
    use Authz::{Authenticated, Scope as ScopeAz};
    const GET: Method = Method::GET;
    // Per-session ownership guard (CCT-420): `machine_uuid -> machines.user_id`,
    // id sourced from the `{id}` path param. `read`/`write` differ only in the
    // recorded `Action` (for CCT-422 RBAC); the owner rule is identical today.
    let sess_read = || Authz::Resource(ResourceKind::Session, Action::Read, IdFrom::Path("id"));
    let sess_write = || Authz::Resource(ResourceKind::Session, Action::Write, IdFrom::Path("id"));
    Routes::new()
        // Version info requires a valid principal — no unauthenticated endpoint
        // survives except `/health`.
        .add(&[GET], "/version", get(routes::web::version), Authn::Bearer, Authenticated)
        .add(
            &[Method::POST],
            "/sessions/register",
            post(routes::sessions::register),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/deregister",
            post(routes::sessions::deregister),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::POST],
            "/sessions/spawn",
            // Multipart spawn with file uploads (CCT-203): the route enforces a
            // 20 MB total cap itself; allow a little headroom over it for
            // multipart framing + base64 isn't applied until after parsing.
            post(routes::spawn::spawn_session).layer(DefaultBodyLimit::max(24 * 1024 * 1024)),
            Authn::Bearer,
            // In-handler machine-owner check (`is_admin || user_id == owner`).
            Authenticated,
        )
        .add(
            // Mid-chat file attachments (CCT-236) — same multipart shape + caps
            // as spawn, same body-limit headroom.
            &[Method::POST],
            "/sessions/{id}/files",
            post(routes::spawn::stage_session_files).layer(DefaultBodyLimit::max(24 * 1024 * 1024)),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::POST],
            "/sessions/dispatch",
            post(routes::dispatch::dispatch),
            Authn::Bearer,
            ScopeAz(auth::Scope::Dispatch),
        )
        .add(
            &[GET],
            "/sessions/dispatchers",
            get(routes::dispatch::list_dispatchers),
            Authn::Bearer,
            // owner_filter() SQL filter in the handler.
            Authenticated,
        )
        // Enrolled-dispatcher management (CCT-285): list with liveness, rename,
        // remove. Enrollment itself is `POST /dispatcher/enroll` below.
        .add(
            &[GET],
            "/dispatchers",
            get(routes::dispatchers::list_dispatchers),
            Authn::Bearer,
            // owner_filter() filter.
            Authenticated,
        )
        .add(
            &[Method::PATCH, Method::DELETE],
            "/dispatchers/{id}",
            patch(routes::dispatchers::update_dispatcher)
                .delete(routes::dispatchers::delete_dispatcher),
            Authn::Bearer,
            ScopeAz(auth::Scope::Enroll),
        )
        .add(
            &[Method::POST],
            "/dispatcher/enroll",
            post(routes::dispatcher::enroll),
            Authn::Bearer,
            ScopeAz(auth::Scope::Enroll),
        )
        // Batch session mutations: owner-filtered in the handler
        // (`filter_owned_ids`), so any authenticated principal is allowed and
        // only their own ids are acted on.
        .add(
            &[Method::POST],
            "/sessions/archive",
            post(routes::sessions::archive_sessions),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::POST],
            "/sessions/unarchive",
            post(routes::sessions::unarchive_sessions),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::POST],
            "/sessions/pin",
            post(routes::sessions::pin_sessions),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::POST],
            "/sessions/unpin",
            post(routes::sessions::unpin_sessions),
            Authn::Bearer,
            Authenticated,
        )
        // Self-scoped list/stats/search endpoints — `owner_filter()` filter in
        // the handler (admin sees all rows, others only their own).
        .add(
            &[GET],
            "/sessions",
            get(routes::sessions::list_sessions),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET],
            "/sessions/stats",
            get(routes::stats::session_stats),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET],
            "/sessions/stats/tokens",
            get(routes::stats::session_token_stats),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET],
            "/sessions/search",
            get(routes::sessions::search_sessions),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET],
            "/sessions/recent-dirs",
            get(routes::stats::recent_dirs),
            Authn::Bearer,
            Authenticated,
        )
        // Per-session routes — ownership enforced by the `Resource(Session)`
        // guard (CCT-420). The `authz_layer` resolves `machine_uuid ->
        // machines.user_id` and applies `admin || owner == caller` before the
        // handler (404 unknown / 403 cross-user). Reads → `Action::Read`,
        // mutations/control → `Action::Write` (the action is recorded for
        // CCT-422 RBAC; the owner rule is identical for both today).
        .add(
            &[GET],
            "/sessions/{id}",
            get(routes::sessions::get_session),
            Authn::Bearer,
            sess_read(),
        )
        .add(
            &[Method::PATCH],
            "/sessions/{id}",
            patch(routes::sessions::rename_session),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[GET],
            "/sessions/{id}/conversation",
            get(routes::sessions::get_conversation),
            Authn::Bearer,
            sess_read(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/message",
            post(routes::sessions::send_message),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/kill",
            post(routes::sessions::kill_session),
            Authn::Bearer,
            sess_write(),
        )
        // Draft sessions (CCT-394): launch promotes a draft to a live spawn
        // (env entered fresh in the body), discard deletes the draft row.
        .add(
            &[Method::POST],
            "/sessions/{id}/launch",
            post(routes::spawn::launch_draft),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/discard",
            post(routes::spawn::discard_draft),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/interrupt",
            post(routes::sessions::interrupt_session),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/resume",
            post(routes::sessions::resume_session),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/set-model",
            post(routes::sessions::set_model),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/switch-account",
            post(routes::sessions::switch_account),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/fork",
            post(routes::sessions::fork_session),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/auto-approve",
            post(routes::sessions::set_auto_approve),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/archive",
            post(routes::sessions::archive_session),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/unarchive",
            post(routes::sessions::unarchive_session),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/pin",
            post(routes::sessions::pin_session),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/unpin",
            post(routes::sessions::unpin_session),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/policy",
            post(routes::sessions::set_session_policy),
            Authn::Bearer,
            sess_write(),
        )
        // Session labels (CCT-360): global label definitions (no owner) +
        // per-session attach/detach (authorize_session in the handler).
        .add(
            &[GET, Method::POST],
            "/labels",
            get(routes::labels::list_labels).post(routes::labels::create_label),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::PATCH, Method::DELETE],
            "/labels/{id}",
            axum::routing::patch(routes::labels::update_label).delete(routes::labels::delete_label),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/labels",
            post(routes::labels::attach_label),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::DELETE],
            "/sessions/{id}/labels/{label_id}",
            axum::routing::delete(routes::labels::detach_label),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[GET],
            "/manifest/daemon",
            get(routes::manifest::daemon_manifest),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET],
            "/daemon/binary/{target}",
            get(routes::manifest::download_daemon_binary),
            Authn::Bearer,
            Authenticated,
        )
        // Prompts: owner_filter() filter in the handler.
        .add(
            &[GET, Method::POST],
            "/prompts",
            get(routes::prompts::list_prompts).post(routes::prompts::create_prompt),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET],
            "/prompts/resolve",
            get(routes::prompts::resolve_prompt),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET, Method::DELETE],
            "/prompts/{id}",
            get(routes::prompts::get_prompt).delete(routes::prompts::delete_prompt),
            Authn::Bearer,
            Authenticated,
        )
        // Provider keys: owner_filter() filter in the handler.
        .add(
            &[GET, Method::POST],
            "/keys",
            get(routes::credentials::list_api_keys).post(routes::credentials::create_api_key),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::DELETE],
            "/keys/{id}",
            delete(routes::credentials::delete_api_key),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET],
            "/keys/{id}/value",
            get(routes::credentials::get_api_key_value),
            Authn::Bearer,
            Authenticated,
        )
        // Accounts: require_human() + owner_filter()/resolve_owner in handler.
        .add(
            &[GET, Method::POST],
            "/accounts",
            get(routes::accounts::list_accounts).post(routes::accounts::create_account),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::POST],
            "/accounts/oauth/start",
            post(routes::accounts::oauth_start),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::POST],
            "/accounts/oauth/finish",
            post(routes::accounts::oauth_finish),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::PATCH, Method::DELETE],
            "/accounts/{id}",
            patch(routes::accounts::rename_account).delete(routes::accounts::delete_account),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET],
            "/accounts/{id}/usage",
            get(routes::accounts::account_usage),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET],
            "/machines/{machine_id}/commands/pending",
            get(routes::spawn::get_machine_commands),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET],
            "/machines/{machine_id}/fs/dirs",
            get(routes::fs::list_dirs),
            Authn::Bearer,
            // Machine-owner guard (CCT-420): `machines.user_id`, id from the
            // `{machine_id}` path param.
            Authz::Resource(ResourceKind::Machine, Action::Read, IdFrom::Path("machine_id")),
        )
        .add(&[GET], "/me", get(routes::me::me), Authn::Bearer, Authenticated)
        .add(&[GET], "/settings", get(routes::settings::get_settings), Authn::Bearer, Authenticated)
        .add(
            &[Method::PUT],
            "/settings",
            put(routes::settings::put_settings),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET],
            "/capabilities",
            get(routes::capabilities::capabilities),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::POST],
            "/enroll",
            post(routes::enroll::enroll),
            Authn::Bearer,
            ScopeAz(auth::Scope::Enroll),
        )
        .add(
            &[Method::POST],
            "/deenroll",
            post(routes::enroll::deenroll),
            Authn::Bearer,
            // In-handler: requires a machine token (machine_id present).
            Authenticated,
        )
        // Admin surface (CCT-410): every route is `forbid_or` (Scope::Admin).
        .add(
            &[Method::POST, GET],
            "/admin/users",
            post(routes::admin_auth::create_user).get(routes::admin_auth::list_users),
            Authn::Bearer,
            ScopeAz(auth::Scope::Admin),
        )
        .add(
            &[Method::DELETE, Method::PATCH],
            "/admin/users/{id}",
            delete(routes::admin_auth::revoke_user).patch(routes::admin_auth::update_user),
            Authn::Bearer,
            ScopeAz(auth::Scope::Admin),
        )
        .add(
            &[Method::DELETE],
            "/admin/users/{id}/purge",
            delete(routes::admin_auth::purge_user),
            Authn::Bearer,
            ScopeAz(auth::Scope::Admin),
        )
        .add(
            &[Method::POST],
            "/admin/users/{id}/rotate",
            post(routes::admin_auth::rotate_user),
            Authn::Bearer,
            ScopeAz(auth::Scope::Admin),
        )
        .add(
            &[GET],
            "/admin/users/{id}/machines",
            get(routes::admin_auth::list_user_machines),
            Authn::Bearer,
            ScopeAz(auth::Scope::Admin),
        )
        .add(
            &[GET],
            "/admin/users/{id}/tokens",
            get(routes::admin_auth::list_user_tokens),
            Authn::Bearer,
            ScopeAz(auth::Scope::Admin),
        )
        .add(
            &[Method::PATCH, Method::DELETE],
            "/admin/users/{id}/tokens/{token_id}",
            patch(routes::admin_auth::relabel_user_token)
                .delete(routes::admin_auth::revoke_user_token),
            Authn::Bearer,
            ScopeAz(auth::Scope::Admin),
        )
        .add(
            &[Method::DELETE],
            "/admin/users/{id}/tokens/{token_id}/purge",
            delete(routes::admin_auth::delete_user_token),
            Authn::Bearer,
            ScopeAz(auth::Scope::Admin),
        )
        .add(
            &[Method::DELETE, Method::PATCH],
            "/admin/machines/{id}",
            delete(routes::admin_auth::revoke_machine).patch(routes::admin_auth::rename_machine),
            Authn::Bearer,
            ScopeAz(auth::Scope::Admin),
        )
        .add(
            &[Method::POST],
            "/admin/machines/{id}/rotate",
            post(routes::admin_auth::rotate_machine),
            Authn::Bearer,
            ScopeAz(auth::Scope::Admin),
        )
        .add(
            &[Method::DELETE],
            "/admin/machines/{id}/purge",
            delete(routes::admin_auth::delete_machine),
            Authn::Bearer,
            ScopeAz(auth::Scope::Admin),
        )
        // Archive: machine/scope checks performed in the handlers.
        .add(&[GET], "/archive/index", get(routes::archive::index), Authn::Bearer, Authenticated)
        .add(
            &[GET],
            "/archive/status",
            get(routes::archive::get_status),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::POST],
            "/archive/manifest",
            post(routes::archive::post_manifest).layer(DefaultBodyLimit::max(8 * 1024 * 1024)),
            Authn::Bearer,
            Authenticated,
        )
        // Rebuild a transcript from stored stream_events (CCT-363). `rebuild`
        // is a literal first segment, so it must NOT collide with the
        // `{project_dir}/{session_id}` matcher below — keep session_id in the
        // query string rather than a second path segment.
        .add(
            &[Method::POST],
            "/archive/rebuild",
            post(routes::archive::rebuild),
            Authn::Bearer,
            Authenticated,
        )
        // Export the caller's archives as a coach-ingestable tar.gz (CCT-364).
        .add(&[GET], "/archive/export", get(routes::archive::export), Authn::Bearer, Authenticated)
        .add(
            &[Method::PUT, Method::HEAD, GET],
            "/archive/{project_dir}/{session_id}",
            put(routes::archive::put)
                .head(routes::archive::head)
                .get(routes::archive::get)
                .layer(DefaultBodyLimit::max(100 * 1024 * 1024)),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET],
            "/permissions/pending",
            get(routes::permissions::list_pending),
            Authn::Bearer,
            // In-handler owner join on session machine_uuid -> machines.user_id.
            Authenticated,
        )
        .add(&[GET], "/skills/index", get(routes::skills::index), Authn::Bearer, Authenticated)
        .add(
            &[Method::PUT, GET],
            "/skills/{name}",
            put(routes::skills::put)
                .get(routes::skills::get)
                .layer(DefaultBodyLimit::max(50 * 1024 * 1024)),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::POST],
            "/users/{id}/tokens",
            post(routes::daemon::mint_user_token),
            Authn::Bearer,
            // In-handler: admin may mint for anyone; a user only for itself.
            Authenticated,
        )
        // CCT-410: per-user scope (ceiling) + per-key (grant) management — all
        // admin-only (`forbid_or`).
        .add(
            &[GET, Method::PATCH],
            "/users/{id}/acls",
            get(routes::admin_auth::get_user_acls).patch(routes::admin_auth::set_user_acls),
            Authn::Bearer,
            ScopeAz(auth::Scope::Admin),
        )
        .add(
            &[GET, Method::POST],
            "/users/{id}/keys",
            get(routes::admin_auth::list_user_keys).post(routes::admin_auth::mint_user_key),
            Authn::Bearer,
            ScopeAz(auth::Scope::Admin),
        )
        .add(
            &[Method::DELETE],
            "/users/{id}/keys/{kid}",
            delete(routes::admin_auth::revoke_user_key),
            Authn::Bearer,
            ScopeAz(auth::Scope::Admin),
        )
        .add(
            &[Method::PATCH],
            "/users/{id}/keys/{kid}/acls",
            patch(routes::admin_auth::set_key_acls),
            Authn::Bearer,
            ScopeAz(auth::Scope::Admin),
        )
}

/// Map the server-private [`auth::AuthContext`] (inserted by `auth_middleware`)
/// into the proto [`cctui_proto::github::CallerIdentity`] the GitHub crate
/// extracts. The GitHub crate must not depend on `cctui-server`, so it cannot
/// see `AuthContext`; this thin layer bridges the two without a dependency cycle.
#[cfg(feature = "github")]
async fn github_identity(
    mut request: axum::extract::Request,
    next: middleware::Next,
) -> axum::response::Response {
    if let Some(ctx) = request.extensions().get::<auth::AuthContext>() {
        let identity = cctui_proto::github::CallerIdentity {
            user_id: Some(ctx.user_id),
            is_admin: ctx.is_admin(),
        };
        request.extensions_mut().insert(identity);
    }
    next.run(request).await
}

async fn init_archive_store() -> Arc<archive_store::ArchiveStore> {
    let root: PathBuf =
        std::env::var("CCTUI_ARCHIVE_PATH").unwrap_or_else(|_| "/archive".into()).into();
    let store = Arc::new(archive_store::ArchiveStore::new(root.clone()));
    if let Err(e) = store.ensure_root().await {
        tracing::warn!(path = %root.display(), "archive root ensure_root failed: {e}");
    }
    store
}

async fn init_skill_store() -> Arc<skill_store::SkillStore> {
    let root: PathBuf =
        std::env::var("CCTUI_SKILLS_PATH").unwrap_or_else(|_| "/skills".into()).into();
    let store = Arc::new(skill_store::SkillStore::new(root.clone()));
    if let Err(e) = store.ensure_root().await {
        tracing::warn!(path = %root.display(), "skill root ensure_root failed: {e}");
    }
    store
}

/// Construct the [`dispatchers::Registry`] of env-configured `http` escape-hatch
/// dispatchers, merged from the legacy `CCTUI_HTTP_DISPATCHERS` and the
/// `kind:"http"` entries of `CCTUI_DISPATCHERS`.
///
/// CCT-292: the in-process `kube`/`docker` dispatchers are gone — production
/// dispatches exclusively through enrolled executor binaries
/// (`/api/v1/dispatcher/ws`), and `resolve_dispatcher` checks enrolled first,
/// falling back to this http-only registry.
async fn init_dispatchers(config: &Config) -> Arc<dispatchers::Registry> {
    let mut registry = dispatchers::Registry::new();

    for d in config.http_dispatchers.iter().chain(config.dispatchers.iter()) {
        tracing::info!(id = %d.id, url = %d.url, "http dispatcher registered");
        registry = registry.with(Arc::new(dispatchers::http::HttpDispatcher::new(
            &d.id,
            &d.url,
            d.token.clone(),
        )));
    }

    Arc::new(registry)
}

#[allow(clippy::cognitive_complexity)]
async fn reaper_task(state: AppState) {
    let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(30));
    loop {
        interval.tick().await;
        let demoted = {
            let mut registry = state.registry.write().await;
            registry.mark_stale(state.config.inactive_after_secs)
        };
        for session_id in &demoted {
            let _ = sqlx::query("UPDATE sessions SET status = 'inactive' WHERE id = $1")
                .bind(session_id.as_str())
                .execute(&state.pool)
                .await;
            tracing::info!(session_id = %session_id, "session demoted to inactive");
        }

        // Auto-archive sessions that have been silent past the TTL so the
        // default list stays self-cleaning. `0` disables it.
        if state.config.archive_after_secs > 0 {
            let cutoff = chrono::Utc::now()
                - chrono::Duration::seconds(
                    i64::try_from(state.config.archive_after_secs).unwrap_or(i64::MAX),
                );
            match sqlx::query(
                // Drafts (CCT-394) are staged-not-running — never auto-archive them.
                "UPDATE sessions SET status = 'archived' \
                 WHERE status NOT IN ('archived', 'draft') AND pinned = false AND last_heartbeat < $1",
            )
            .bind(cutoff)
            .execute(&state.pool)
            .await
            {
                Ok(res) if res.rows_affected() > 0 => {
                    tracing::info!(count = res.rows_affected(), "auto-archived stale sessions");
                }
                Ok(_) => {}
                Err(err) => tracing::warn!(%err, "auto-archive sweep failed"),
            }
        }

        // Soft-delete ephemeral (dispatch/worker) machines that have gone
        // quiet past the TTL — pods that died before self-deenroll (CCT-183).
        // Mirrors the self-deenroll write (revoked_at + deleted_at) so the row
        // survives for historical session FKs but drops out of every listing.
        if state.config.ephemeral_machine_ttl_secs > 0 {
            let cutoff = chrono::Utc::now()
                - chrono::Duration::seconds(
                    i64::try_from(state.config.ephemeral_machine_ttl_secs).unwrap_or(i64::MAX),
                );
            match sqlx::query(
                "UPDATE machines SET revoked_at = COALESCE(revoked_at, now()), deleted_at = now() \
                 WHERE kind = 'ephemeral' AND deleted_at IS NULL AND last_seen_at < $1",
            )
            .bind(cutoff)
            .execute(&state.pool)
            .await
            {
                Ok(res) if res.rows_affected() > 0 => {
                    tracing::info!(count = res.rows_affected(), "reaped stale ephemeral machines");
                }
                Ok(_) => {}
                Err(err) => tracing::warn!(%err, "ephemeral machine reap failed"),
            }
        }

        // Ephemeral dispatch keys (CCT-296): per-session credentials handed to
        // worker pods. Revoke a key once its bound session reaches the terminal
        // `archived` state (blast radius dies with the session, ahead of TTL),
        // and hard-delete keys past their `expires_at` so the table stays clean.
        // The auth path already rejects revoked/expired keys; this just keeps
        // the rows from accumulating and tightens revocation to session end.
        match sqlx::query(
            "UPDATE auth_keys SET revoked_at = now() \
             WHERE kind = 'ephemeral' AND revoked_at IS NULL AND session_id IN \
               (SELECT id FROM sessions WHERE status = 'archived')",
        )
        .execute(&state.pool)
        .await
        {
            Ok(res) if res.rows_affected() > 0 => {
                tracing::info!(
                    count = res.rows_affected(),
                    "revoked ephemeral dispatch keys for archived sessions"
                );
            }
            Ok(_) => {}
            Err(err) => tracing::warn!(%err, "ephemeral key revoke sweep failed"),
        }
        match sqlx::query(
            "DELETE FROM auth_keys \
             WHERE kind = 'ephemeral' AND expires_at IS NOT NULL AND expires_at < now()",
        )
        .execute(&state.pool)
        .await
        {
            Ok(res) if res.rows_affected() > 0 => {
                tracing::info!(
                    count = res.rows_affected(),
                    "deleted expired ephemeral dispatch keys"
                );
            }
            Ok(_) => {}
            Err(err) => tracing::warn!(%err, "expired ephemeral key delete sweep failed"),
        }

        // Machine liveness (CCT-255): re-derive every machine's tier from its
        // `last_seen_at` and broadcast any transitions. The 30s cadence means a
        // daemon that stops heartbeating ages online → stale → offline on its
        // own — the acceptance case "killing a daemon flips it offline within
        // one liveness window without a dispatch attempt".
        machine_liveness::sweep(&state).await;
        machine_liveness::sweep_dispatchers(&state).await;

        // Completion webhooks (CCT-294): fire a server-side callback for any
        // dispatched session that has reached a terminal state — the
        // crash-coverage path the worker's REPLY_URL exit trap can miss.
        webhook::sweep(&state).await;

        {
            let mut pstore = state.permission_store.write().await;
            pstore.reap_stale(300); // 5 minutes
        }
    }
}
