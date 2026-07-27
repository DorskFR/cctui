mod auth;
mod authz;
mod bandwidth_watch;
mod bus;
mod config;
mod crypto;
mod db;
mod dispatchers;
mod langfuse;
mod machine_liveness;
mod normalize;
mod ntfy;
mod openapi;
mod policy;
mod presence;
mod registry;
mod routes;
mod settings_catalog;
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
    // One-release back-compat shim: if the retired
    // CCTUI_CLAUDE_LITELLM_* env vars are set, synthesize a managed (read-only)
    // anthropic-compatible account per user so existing deployments keep working
    // until they migrate to first-class accounts.
    routes::accounts::sync_litellm_shim(&pool, &config).await;
    let auth_config = auth::AuthConfig::new(Config::admin_tokens(), pool.clone());
    // resolve CCTUI_ADMIN_TOKENS to a seeded admin user + api_keys rows
    // with {admin} ceiling/grant, so the break-glass token is a real identity
    // rather than a user_id=None ghost. Idempotent, best-effort.
    auth_config.seed_admin().await;

    let skills = init_skill_store().await;
    let dispatchers = init_dispatchers(&config);

    let presence = Arc::new(presence::PodIdentity::from_env());
    let http_client = reqwest::Client::new();

    // Bus transport selection: with a routable pod IP this replica
    // participates in the peer mesh — mint/load the internal shared secret and
    // route/relay through `PeerHttpTransport`. Without one (local dev, single
    // replica) the bus stays local-only (`NoopTransport`) and writes nothing.
    let (transport, internal_secret): (Box<dyn bus::Transport>, Option<Arc<str>>) =
        if presence.ip.is_some() {
            let secret = routes::internal::ensure_secret(&pool).await?;
            let transport = bus::peer::PeerHttpTransport::new(
                pool.clone(),
                http_client.clone(),
                presence.pod.clone(),
                config.port,
                secret.clone(),
            );
            (Box::new(transport), Some(Arc::from(secret.as_str())))
        } else {
            (Box::new(bus::NoopTransport), None)
        };

    let state = AppState {
        pool,
        config: config.clone(),
        registry: Registry::shared(),
        permission_store: routes::permissions::PermissionStore::shared(),
        // The single routing seam for daemon/dispatcher WS traffic;
        // the transport behind it is chosen above.
        bus: bus::Bus::new(transport),
        auth_config: auth_config.clone(),
        skills,
        presence,
        internal_secret,
        dispatcher_liveness: Arc::new(dashmap::DashMap::new()),
        dispatchers,
        machine_liveness: Arc::new(dashmap::DashMap::new()),
        account_locks: Arc::new(dashmap::DashMap::new()),
        http_client,
        // Optional Langfuse tracing sink. `None` (dark) unless the
        // CCTUI_LANGFUSE_* env is fully set — zero overhead on the gateway path.
        langfuse: langfuse::LangfuseConfig::from_env()
            .map(|c| Arc::new(langfuse::LangfuseClient::new(c, reqwest::Client::new()))),
        pending_oauth_logins: Arc::new(dashmap::DashMap::new()),
        account_usage_cache: Arc::new(dashmap::DashMap::new()),
        pr_status_cache: cctui_proto::classifier::PrStatusCache::new(),
        soft_limit_blocked: Arc::new(dashmap::DashMap::new()),
        gateway_orphan_spam: Arc::new(dashmap::DashMap::new()),
        account_reauth: Arc::new(dashmap::DashMap::new()),
        codex_catalogs: Arc::new(dashmap::DashMap::new()),
        eviction_tracker: Arc::new(bandwidth_watch::EvictionTracker::default()),
        divergence_tracker: Arc::new(bandwidth_watch::DivergenceTracker::default()),
        machine_event_inserts: Arc::new(dashmap::DashMap::new()),
    };

    // Warm the reauth gate from the persisted flag so a restart doesn't
    // strand an account: without this the success path couldn't clear a flag set
    // before the restart (it only writes on the in-memory transition).
    if let Ok(ids) =
        sqlx::query_scalar::<_, uuid::Uuid>("SELECT id FROM account_providers WHERE needs_reauth")
            .fetch_all(&state.pool)
            .await
    {
        for id in ids {
            state.account_reauth.insert(id, ());
        }
    }

    // Replica-aware WS presence: registered only when the pod knows
    // its routable IP; the heartbeat task keeps this pod's rows trusted and
    // reaps rows crashed pods left behind.
    if state.presence.ip.is_some() {
        tokio::spawn(presence::heartbeat_task(state.clone()));
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

    // Optional GitHub integration. Behind the `github`
    // Cargo feature: run its embedded migrations and merge its routes. A build
    // without the feature contains zero GitHub code, routes, or schema.
    #[cfg(feature = "github")]
    cctui_github::migrate(&state.pool).await?;

    // `{id}` etc. in route paths are axum path-param syntax, not format args.
    #[allow(clippy::literal_string_with_formatting_args)]
    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        // Self-describing API surface. Both are unauthenticated meta
        // routes — like `/health` — because they expose ONLY the public shape of
        // the API (paths/methods/auth model/summaries), never any data. An agent
        // handed a base URL can discover the surface, then authenticate.
        .route("/llms.txt", get(openapi::llms_txt))
        .route("/api/v1/openapi.json", get(openapi::openapi_json))
        .route("/api/v1/ws", get(ws::tui_ws))
        // Browser auth-cookie endpoints. Self-authenticating: `login`
        // validates the presented token and sets the `HttpOnly` cookie, `logout`
        // clears it — both live outside the `auth_middleware` group.
        .route("/api/v1/auth/login", post(routes::auth::login))
        .route("/api/v1/auth/logout", post(routes::auth::logout))
        // Daemon-facing endpoints. `auth` and `ws` carry their own auth
        // (machine-key Bearer / `?token=` query) so they live outside the
        // user-token-only `api_router` group.
        .route("/api/v1/daemon/auth", post(routes::daemon::auth))
        .route("/api/v1/daemon/ws", get(routes::daemon::ws))
        // Launch-time gateway-env pull: the daemon resolves a
        // session's account env here on every worker (re)launch. Self-auths via
        // the machine-key Bearer, so it sits beside the other daemon endpoints.
        .route("/api/v1/daemon/sessions/{id}/gateway-env", get(routes::daemon::session_gateway_env))
        // Token-validity probe: the daemon's low-frequency sweep asks
        // whether the session token it launched a trusted worker with still
        // resolves (by sha256 hash — no token material on the wire). Same
        // machine-key self-auth as gateway-env.
        .route("/api/v1/daemon/sessions/{id}/token-valid", get(routes::daemon::session_token_valid))
        // Agent-posted image upload: the daemon POSTs raw image bytes
        // it detected as a marker in an assistant message. Self-auths via the
        // machine-key Bearer like the sibling daemon endpoints, so it sits here
        // outside the user-token `api_router`. Cap the body a little over the
        // 5 MiB per-image limit so an over-cap upload 413s in-handler.
        .route(
            "/api/v1/daemon/sessions/{id}/images",
            post(routes::images::upload_session_image)
                .layer(DefaultBodyLimit::max(6 * 1024 * 1024)),
        )
        // Content-addressed blob upload: the daemon PUTs oversized
        // base64 attachments it extracted from transcript payloads, keyed by
        // sha256. Machine-key Bearer self-auth, so it sits beside the other
        // daemon endpoints outside the user-token `api_router`. Headroom over
        // the per-blob cap so an over-cap upload 413s in-handler.
        .route(
            "/api/v1/daemon/blobs/{hash}",
            put(routes::blobs::put_blob)
                .layer(DefaultBodyLimit::max(routes::blobs::MAX_BLOB_BYTES + 1024 * 1024)),
        )
        // Enrolled-dispatcher endpoints. Carry their own key auth
        // (dispatcher-key Bearer / `?token=`), so they live outside the
        // user-token `api_router` group, like the daemon endpoints.
        .route("/api/v1/dispatcher/auth", post(routes::dispatcher::auth))
        .route("/api/v1/dispatcher/ws", get(routes::dispatcher::ws))
        .route("/api/v1/triggers/{kind}", post(routes::triggers::ingest))
        // OAuth passthrough gateway. Auths via the session-scoped
        // token in the request's own Authorization header — NOT the user-token
        // `api_router` middleware — so it lives on the outer app. Matches any
        // method + sub-path under each provider prefix.
        .route("/gateway/anthropic/{*path}", any(routes::gateway::anthropic))
        .route("/gateway/openai/{*path}", any(routes::gateway::openai))
        // Pod-to-pod bus endpoints. Self-authenticating via the
        // cluster-internal shared secret (constant-time compare; user/machine
        // tokens never accepted), so they live outside the `api_router` auth
        // group. `route` may carry a forwarded stage-files upload — give it the
        // same body headroom as the spawn/files routes it serves.
        .route(
            "/internal/bus/route",
            post(routes::internal::bus_route).layer(DefaultBodyLimit::max(32 * 1024 * 1024)),
        )
        .route("/internal/bus/publish", post(routes::internal::bus_publish))
        .nest("/api/v1", api_router)
        // The web UI is served same-origin in prod, so the `HttpOnly` auth
        // cookie flows without any cross-origin credential config.
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
            state.bus.server_sender(),
            state.pr_status_cache.clone(),
        )
        .layer(middleware::from_fn(github_identity))
        .layer(middleware::from_fn(auth::auth_middleware))
        .layer(Extension(auth_config.clone())),
    );

    // the agent MCP review endpoint authenticates the bearer session
    // token on its own (it is not a user/machine token `auth_middleware` knows),
    // so it is merged WITHOUT the auth/identity layers above.
    #[cfg(feature = "github")]
    let app = app.nest(
        "/api/v1",
        cctui_github::mcp_routes(
            state.pool.clone(),
            state.bus.server_sender(),
            state.pr_status_cache.clone(),
        ),
    );

    // the reconcile poll loop. A background task (mirroring
    // `reaper_task`) that heals missed webhooks and hydrates first install by
    // polling GitHub for "PRs involving me" per connector. Behind the `github`
    // feature; disabled when `CCTUI_GITHUB_RECONCILE_SECS=0`.
    #[cfg(feature = "github")]
    cctui_github::spawn_reconcile(
        state.pool.clone(),
        state.bus.server_sender(),
        state.pr_status_cache.clone(),
    );

    tokio::spawn(reaper_task(state));

    let listener = tokio::net::TcpListener::bind(config.bind_addr()).await?;
    tracing::info!("listening on {}", config.bind_addr());
    axum::serve(listener, app).await?;
    Ok(())
}

/// Build the `/api/v1` route table from the descriptor list. Every
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
    // Per-session ownership guard: `machine_uuid -> machines.user_id`,
    // id sourced from the `{id}` path param. `read`/`write` differ only in the
    // recorded `Action` (for RBAC); the owner rule is identical today.
    let sess_read = || Authz::Resource(ResourceKind::Session, Action::Read, IdFrom::Path("id"));
    let sess_write = || Authz::Resource(ResourceKind::Session, Action::Write, IdFrom::Path("id"));
    Routes::new()
        // Version info requires a valid principal — no unauthenticated endpoint
        // survives except `/health`.
        .add(
            &[GET],
            "/version",
            "Server version and build info.",
            get(routes::web::version),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::POST],
            "/sessions/register",
            "Register a session the daemon just launched.",
            post(routes::sessions::register),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/deregister",
            "Deregister a session (mark it gone).",
            post(routes::sessions::deregister),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::POST],
            "/sessions/spawn",
            "Spawn a new session on a machine, with optional file uploads.",
            // Multipart spawn with file uploads: the route enforces a
            // 20 MB total cap itself; allow a little headroom over it for
            // multipart framing + base64 isn't applied until after parsing.
            post(routes::spawn::spawn_session).layer(DefaultBodyLimit::max(24 * 1024 * 1024)),
            Authn::Bearer,
            // In-handler machine-owner check (`is_admin || user_id == owner`).
            Authenticated,
        )
        .add(
            // Mid-chat file attachments — same multipart shape + caps
            // as spawn, same body-limit headroom.
            &[Method::POST],
            "/sessions/{id}/files",
            "Attach files to a live session mid-conversation.",
            post(routes::spawn::stage_session_files).layer(DefaultBodyLimit::max(24 * 1024 * 1024)),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::POST],
            "/sessions/dispatch",
            "Dispatch a session to an enrolled executor (remote runner).",
            post(routes::dispatch::dispatch),
            Authn::Bearer,
            ScopeAz(auth::Scope::Dispatch),
        )
        .add(
            &[GET],
            "/sessions/dispatchers",
            "List dispatch targets available for a spawn.",
            get(routes::dispatch::list_dispatchers),
            Authn::Bearer,
            // owner_filter() SQL filter in the handler.
            Authenticated,
        )
        // Enrolled-dispatcher management: list with liveness, rename,
        // remove. Enrollment itself is `POST /dispatcher/enroll` below.
        .add(
            &[GET],
            "/dispatchers",
            "List enrolled dispatchers with liveness.",
            get(routes::dispatchers::list_dispatchers),
            Authn::Bearer,
            // owner_filter() filter.
            Authenticated,
        )
        .add(
            &[Method::PATCH, Method::DELETE],
            "/dispatchers/{id}",
            "Rename or remove an enrolled dispatcher.",
            patch(routes::dispatchers::update_dispatcher)
                .delete(routes::dispatchers::delete_dispatcher),
            Authn::Bearer,
            ScopeAz(auth::Scope::Enroll),
        )
        .add(
            &[Method::POST],
            "/dispatcher/enroll",
            "Enroll a new dispatcher (executor) and mint its key.",
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
            "Archive a batch of sessions by id.",
            post(routes::sessions::archive_sessions),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::POST],
            "/sessions/unarchive",
            "Unarchive a batch of sessions by id.",
            post(routes::sessions::unarchive_sessions),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::POST],
            "/sessions/pin",
            "Pin a batch of sessions by id.",
            post(routes::sessions::pin_sessions),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::POST],
            "/sessions/unpin",
            "Unpin a batch of sessions by id.",
            post(routes::sessions::unpin_sessions),
            Authn::Bearer,
            Authenticated,
        )
        // Self-scoped list/stats/search endpoints — `owner_filter()` filter in
        // the handler (admin sees all rows, others only their own).
        .add(
            &[GET],
            "/sessions",
            "List your sessions (admin: all).",
            get(routes::sessions::list_sessions),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET],
            "/sessions/stats",
            "Aggregate session counts/status stats.",
            get(routes::stats::session_stats),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET],
            "/sessions/stats/tokens",
            "Token-usage stats across sessions.",
            get(routes::stats::session_token_stats),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET],
            "/sessions/stats/usage",
            "Overview usage analytics: tokens over time, per-model, heatmap.",
            get(routes::stats::session_usage_analytics),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET],
            "/sessions/search",
            "Full-text search across your sessions.",
            get(routes::sessions::search_sessions),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET],
            "/sessions/search/values",
            "Autocomplete values for a search field.",
            get(routes::sessions::search_field_values),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET],
            "/sessions/recent-dirs",
            "List recently used working directories.",
            get(routes::stats::recent_dirs),
            Authn::Bearer,
            Authenticated,
        )
        // Per-session routes — ownership enforced by the `Resource(Session)`
        // guard. The `authz_layer` resolves `machine_uuid ->
        // machines.user_id` and applies `admin || owner == caller` before the
        // handler (404 unknown / 403 cross-user). Reads → `Action::Read`,
        // mutations/control → `Action::Write` (the action is recorded for
        // RBAC; the owner rule is identical for both today).
        .add(
            &[GET],
            "/sessions/{id}",
            "Get one session's details.",
            get(routes::sessions::get_session),
            Authn::Bearer,
            sess_read(),
        )
        .add(
            &[Method::PATCH],
            "/sessions/{id}",
            "Rename a session.",
            patch(routes::sessions::rename_session),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[GET],
            "/sessions/{id}/conversation",
            "Fetch a session's normalized conversation transcript.",
            get(routes::sessions::get_conversation),
            Authn::Bearer,
            sess_read(),
        )
        .add(
            &[GET],
            "/sessions/{id}/images/{image_id}",
            "Fetch an agent-posted image blob (CCT-566).",
            get(routes::images::get_session_image),
            Authn::Bearer,
            sess_read(),
        )
        .add(
            &[GET],
            "/sessions/{id}/blobs/{hash}",
            "Resolve a content-addressed embedded-attachment blob (CCT-739).",
            get(routes::blobs::get_blob),
            Authn::Bearer,
            sess_read(),
        )
        .add(
            &[GET],
            "/sessions/{id}/diagnose",
            "Snapshot everything the daemon knows about a session, dated (CCT-547).",
            get(routes::diagnose::diagnose_session),
            Authn::Bearer,
            sess_read(),
        )
        .add(
            &[GET],
            "/sessions/{id}/langfuse",
            "Langfuse cost/usage rollup for a session (CCT-564).",
            get(routes::langfuse::session_langfuse),
            Authn::Bearer,
            sess_read(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/message",
            "Send a message to a live session.",
            post(routes::sessions::send_message),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/kill",
            "Kill a session's underlying process.",
            post(routes::sessions::kill_session),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/seen",
            "Mark this session's messages seen for the caller (CCT-580).",
            post(routes::sessions::mark_seen),
            Authn::Bearer,
            sess_write(),
        )
        // Draft sessions: launch promotes a draft to a live spawn
        // (env entered fresh in the body), discard deletes the draft row.
        .add(
            &[Method::POST],
            "/sessions/{id}/launch",
            "Launch a draft session into a live spawn.",
            post(routes::spawn::launch_draft),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/discard",
            "Discard a draft session.",
            post(routes::spawn::discard_draft),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/interrupt",
            "Interrupt a session's current turn.",
            post(routes::sessions::interrupt_session),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/resume",
            "Resume an exited session.",
            post(routes::sessions::resume_session),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/set-model",
            "Change a session's model.",
            post(routes::sessions::set_model),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/switch-account",
            "Switch the account backing a session.",
            post(routes::sessions::switch_account),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[GET],
            "/sessions/{id}/bindings",
            "List a session's per-family account bindings.",
            get(routes::sessions::session_bindings),
            Authn::Bearer,
            sess_read(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/fork",
            "Fork a session into a new one.",
            post(routes::sessions::fork_session),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/auto-approve",
            "Toggle auto-approval of tool-use for a session.",
            post(routes::sessions::set_auto_approve),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/archive",
            "Archive a single session.",
            post(routes::sessions::archive_session),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/unarchive",
            "Unarchive a single session.",
            post(routes::sessions::unarchive_session),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/pin",
            "Pin a single session.",
            post(routes::sessions::pin_session),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/unpin",
            "Unpin a single session.",
            post(routes::sessions::unpin_session),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/policy",
            "Set a session's permission policy.",
            post(routes::sessions::set_session_policy),
            Authn::Bearer,
            sess_write(),
        )
        // Session labels: global label definitions (no owner) +
        // per-session attach/detach (authorize_session in the handler).
        .add(
            &[GET, Method::POST],
            "/labels",
            "List label definitions, or create one.",
            get(routes::labels::list_labels).post(routes::labels::create_label),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::PATCH, Method::DELETE],
            "/labels/{id}",
            "Rename or delete a label definition.",
            axum::routing::patch(routes::labels::update_label).delete(routes::labels::delete_label),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/labels",
            "Attach a label to a session.",
            post(routes::labels::attach_label),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::DELETE],
            "/sessions/{id}/labels/{label_id}",
            "Detach a label from a session.",
            axum::routing::delete(routes::labels::detach_label),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[GET],
            "/manifest/daemon",
            "Daemon update manifest (latest version + download URLs).",
            get(routes::manifest::daemon_manifest),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET],
            "/daemon/binary/{target}",
            "Download a daemon binary for a target (self-update proxy).",
            get(routes::manifest::download_daemon_binary),
            Authn::Bearer,
            Authenticated,
        )
        // Prompts: owner_filter() filter in the handler.
        .add(
            &[GET, Method::POST],
            "/prompts",
            "List your saved prompts, or create one.",
            get(routes::prompts::list_prompts).post(routes::prompts::create_prompt),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET],
            "/prompts/resolve",
            "Resolve a prompt by name/reference.",
            get(routes::prompts::resolve_prompt),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET, Method::DELETE],
            "/prompts/{id}",
            "Get or delete a saved prompt.",
            get(routes::prompts::get_prompt).delete(routes::prompts::delete_prompt),
            Authn::Bearer,
            Authenticated,
        )
        // Provider keys: owner_filter() filter in the handler.
        .add(
            &[GET, Method::POST],
            "/keys",
            "List your provider API keys, or store a new one.",
            get(routes::credentials::list_api_keys).post(routes::credentials::create_api_key),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::DELETE],
            "/keys/{id}",
            "Delete a stored provider API key.",
            delete(routes::credentials::delete_api_key),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET],
            "/keys/{id}/value",
            "Reveal a stored provider API key's value.",
            get(routes::credentials::get_api_key_value),
            Authn::Bearer,
            Authenticated,
        )
        // Accounts: require_human() + owner_filter()/resolve_owner in handler.
        .add(
            &[GET],
            "/accounts/settings-catalog",
            "The per-account settings catalog (exposable keys, env allowlist, preset).",
            get(routes::accounts::settings_catalog),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET, Method::POST],
            "/accounts",
            "List your accounts (identities + provider credentials), or create one.",
            get(routes::accounts::list_accounts).post(routes::accounts::create_account),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::POST],
            "/accounts/oauth/start",
            "Begin an OAuth account authorization flow.",
            post(routes::accounts::oauth_start),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::POST],
            "/accounts/oauth/finish",
            "Complete an OAuth account authorization flow.",
            post(routes::accounts::oauth_finish),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET, Method::PATCH, Method::DELETE],
            "/accounts/{id}",
            "Get, rename/re-env, or delete an account identity.",
            get(routes::accounts::get_account)
                .patch(routes::accounts::update_account)
                .delete(routes::accounts::delete_account),
            Authn::Bearer,
            Authenticated,
        )
        // Provider credentials under an account identity: owner-scoped
        // in the handlers like the other account routes.
        .add(
            &[Method::POST],
            "/accounts/{id}/providers",
            "Attach a provider credential to an account.",
            post(routes::accounts::add_provider),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::PATCH, Method::DELETE],
            "/accounts/{id}/providers/{provider_id}",
            "Edit or remove one of an account's provider credentials.",
            patch(routes::accounts::update_provider).delete(routes::accounts::delete_provider),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::POST],
            "/accounts/{id}/providers/{provider_id}/move",
            "Move a provider credential to another account of the same owner.",
            post(routes::accounts::move_provider),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET],
            "/accounts/{id}/usage",
            "Get an account's usage/limits.",
            get(routes::accounts::account_usage),
            Authn::Bearer,
            Authenticated,
        )
        // Account sharing management: owner-scoped in the handler
        // (require_account_owner) just like the other account routes.
        .add(
            &[GET, Method::POST],
            "/accounts/{id}/shares",
            "List or grant shares of an account to other users.",
            get(routes::accounts::list_shares).post(routes::accounts::grant_share),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::DELETE],
            "/accounts/{id}/shares/{user_id}",
            "Revoke a user's share of an account.",
            delete(routes::accounts::revoke_share),
            Authn::Bearer,
            Authenticated,
        )
        // Generic resource-sharing CRUD: owner-scoped in the handler
        // (require_owner) for any shareable kind. The account routes above are
        // static-path back-compat aliases; these serve machine/dispatcher/etc.
        .add(
            &[GET, Method::POST],
            "/{resource_type}/{id}/shares",
            "List or grant shares of a resource to other users.",
            get(routes::shares::list_shares).post(routes::shares::grant_share),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::DELETE],
            "/{resource_type}/{id}/shares/{user_id}",
            "Revoke a user's share of a resource.",
            delete(routes::shares::revoke_share),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET],
            "/machines/{machine_id}/commands/pending",
            "Poll a machine's pending spawn/control commands.",
            get(routes::spawn::get_machine_commands),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET],
            "/machines/{machine_id}/fs/dirs",
            "List directories on a machine (spawn dir picker).",
            get(routes::fs::list_dirs),
            Authn::Bearer,
            // Machine-owner guard: `machines.user_id`, id from the
            // `{machine_id}` path param.
            Authz::Resource(ResourceKind::Machine, Action::Read, IdFrom::Path("machine_id")),
        )
        .add(
            &[GET],
            "/machines/{machine_id}/codex-models",
            "Machine/account-scoped codex model catalog (CCT-641).",
            get(routes::codex_models::get_codex_models),
            Authn::Bearer,
            Authz::Resource(ResourceKind::Machine, Action::Read, IdFrom::Path("machine_id")),
        )
        .add(
            &[GET],
            "/me",
            "Get the current principal (user, scopes, machine).",
            get(routes::me::me),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET],
            "/settings",
            "Get your user settings.",
            get(routes::settings::get_settings),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::PUT],
            "/settings",
            "Replace your user settings.",
            put(routes::settings::put_settings),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::POST],
            "/settings/rescrub",
            "Re-apply the secret-scrub list to your stored events (CCT-731).",
            post(routes::settings::rescrub_settings),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET],
            "/capabilities",
            "List server capabilities/feature flags.",
            get(routes::capabilities::capabilities),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::POST],
            "/enroll",
            "Enroll this machine and mint its machine key.",
            post(routes::enroll::enroll),
            Authn::Bearer,
            ScopeAz(auth::Scope::Enroll),
        )
        .add(
            &[GET],
            "/machines/{machine_id}/status",
            "Machine connectivity/liveness snapshot (remote-enroll verification).",
            get(routes::enroll::machine_status),
            Authn::Bearer,
            Authz::Resource(ResourceKind::Machine, Action::Read, IdFrom::Path("machine_id")),
        )
        .add(
            &[Method::POST],
            "/deenroll",
            "Deenroll the current machine.",
            post(routes::enroll::deenroll),
            Authn::Bearer,
            // In-handler: requires a machine token (machine_id present).
            Authenticated,
        )
        // Admin surface: every route is `forbid_or` (Scope::Admin).
        .add(
            &[Method::POST, GET],
            "/admin/users",
            "List all users, or create a user (admin).",
            post(routes::admin_auth::create_user).get(routes::admin_auth::list_users),
            Authn::Bearer,
            ScopeAz(auth::Scope::Admin),
        )
        .add(
            &[Method::DELETE, Method::PATCH],
            "/admin/users/{id}",
            "Revoke or update a user (admin).",
            delete(routes::admin_auth::revoke_user).patch(routes::admin_auth::update_user),
            Authn::Bearer,
            ScopeAz(auth::Scope::Admin),
        )
        .add(
            &[Method::DELETE],
            "/admin/users/{id}/purge",
            "Hard-delete a user and all their data (admin).",
            delete(routes::admin_auth::purge_user),
            Authn::Bearer,
            ScopeAz(auth::Scope::Admin),
        )
        .add(
            &[Method::POST],
            "/admin/users/{id}/rotate",
            "Rotate a user's tokens (admin).",
            post(routes::admin_auth::rotate_user),
            Authn::Bearer,
            ScopeAz(auth::Scope::Admin),
        )
        .add(
            &[GET],
            "/admin/users/{id}/machines",
            "List a user's machines (admin).",
            get(routes::admin_auth::list_user_machines),
            Authn::Bearer,
            ScopeAz(auth::Scope::Admin),
        )
        .add(
            &[GET],
            "/admin/users/{id}/tokens",
            "List a user's tokens (admin).",
            get(routes::admin_auth::list_user_tokens),
            Authn::Bearer,
            ScopeAz(auth::Scope::Admin),
        )
        .add(
            &[Method::PATCH, Method::DELETE],
            "/admin/users/{id}/tokens/{token_id}",
            "Relabel or revoke a user's token (admin).",
            patch(routes::admin_auth::relabel_user_token)
                .delete(routes::admin_auth::revoke_user_token),
            Authn::Bearer,
            ScopeAz(auth::Scope::Admin),
        )
        .add(
            &[Method::DELETE],
            "/admin/users/{id}/tokens/{token_id}/purge",
            "Hard-delete a user's token (admin).",
            delete(routes::admin_auth::delete_user_token),
            Authn::Bearer,
            ScopeAz(auth::Scope::Admin),
        )
        .add(
            &[Method::DELETE, Method::PATCH],
            "/admin/machines/{id}",
            "Revoke or rename a machine (admin).",
            delete(routes::admin_auth::revoke_machine).patch(routes::admin_auth::rename_machine),
            Authn::Bearer,
            ScopeAz(auth::Scope::Admin),
        )
        .add(
            &[Method::POST],
            "/admin/machines/{id}/rotate",
            "Rotate a machine's key (admin).",
            post(routes::admin_auth::rotate_machine),
            Authn::Bearer,
            ScopeAz(auth::Scope::Admin),
        )
        .add(
            &[Method::DELETE],
            "/admin/machines/{id}/purge",
            "Hard-delete a machine (admin).",
            delete(routes::admin_auth::delete_machine),
            Authn::Bearer,
            ScopeAz(auth::Scope::Admin),
        )
        .add(
            &[GET],
            "/permissions/pending",
            "List pending tool-use permission requests.",
            get(routes::permissions::list_pending),
            Authn::Bearer,
            // In-handler owner join on session machine_uuid -> machines.user_id.
            Authenticated,
        )
        .add(
            &[GET],
            "/skills/index",
            "List available skills.",
            get(routes::skills::index),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::PUT, GET],
            "/skills/{name}",
            "Upload or fetch a skill bundle by name.",
            put(routes::skills::put)
                .get(routes::skills::get)
                .layer(DefaultBodyLimit::max(50 * 1024 * 1024)),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::POST],
            "/users/{id}/tokens",
            "Mint a token for a user (self or admin).",
            post(routes::daemon::mint_user_token),
            Authn::Bearer,
            // In-handler: admin may mint for anyone; a user only for itself.
            Authenticated,
        )
        // per-user scope (ceiling) + per-key (grant) management — all
        // admin-only (`forbid_or`).
        .add(
            &[GET, Method::PATCH],
            "/users/{id}/acls",
            "Get or set a user's scope ceiling (admin).",
            get(routes::admin_auth::get_user_acls).patch(routes::admin_auth::set_user_acls),
            Authn::Bearer,
            ScopeAz(auth::Scope::Admin),
        )
        .add(
            &[GET, Method::POST],
            "/users/{id}/keys",
            "List or mint a user's API keys (admin).",
            get(routes::admin_auth::list_user_keys).post(routes::admin_auth::mint_user_key),
            Authn::Bearer,
            ScopeAz(auth::Scope::Admin),
        )
        .add(
            &[Method::DELETE],
            "/users/{id}/keys/{kid}",
            "Revoke a user's API key (admin).",
            delete(routes::admin_auth::revoke_user_key),
            Authn::Bearer,
            ScopeAz(auth::Scope::Admin),
        )
        .add(
            &[Method::PATCH],
            "/users/{id}/keys/{kid}/acls",
            "Set a key's scope grant (admin).",
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
/// The in-process `kube`/`docker` dispatchers are gone — production
/// dispatches exclusively through enrolled executor binaries
/// (`/api/v1/dispatcher/ws`), and `resolve_dispatcher` checks enrolled first,
/// falling back to this http-only registry.
fn init_dispatchers(config: &Config) -> Arc<dispatchers::Registry> {
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
                // Drafts are staged-not-running — never auto-archive them.
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
        // quiet past the TTL — pods that died before self-deenroll.
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

        // Ephemeral dispatch keys: per-session credentials handed to
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

        // Machine liveness: re-derive every machine's tier from its
        // `last_seen_at` and broadcast any transitions. The 30s cadence means a
        // daemon that stops heartbeating ages online → stale → offline on its
        // own — the acceptance case "killing a daemon flips it offline within
        // one liveness window without a dispatch attempt".
        machine_liveness::sweep(&state).await;
        machine_liveness::sweep_dispatchers(&state).await;

        // Completion webhooks: fire a server-side callback for any
        // dispatched session that has reached a terminal state — the
        // crash-coverage path the worker's REPLY_URL exit trap can miss.
        webhook::sweep(&state).await;

        {
            let mut pstore = state.permission_store.write().await;
            pstore.reap_stale(300); // 5 minutes
        }
    }
}
