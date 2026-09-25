mod account_pick;
mod account_resolve;
mod api_routes;
mod auth;
mod authz;
mod auto_archive;
mod auto_resume;
mod bandwidth_watch;
mod brief;
mod bus;
mod cache_bust;
mod config;
mod cost;
mod crypto;
mod db;
mod dispatchers;
mod error;
mod fireworks_billing;
mod followup;
mod http_cache;
mod keepalive;
mod langfuse;
mod live_sessions;
mod machine_liveness;
mod machine_resources;
mod normalize;
mod ntfy;
mod openapi;
mod outbound;
mod pace;
mod policy;
mod pool_usage;
mod presence;
mod registry;
mod routes;
mod scheduled_messages;
mod session_emoji;
mod settings_catalog;
mod skill_store;
mod soft_limit;
mod spawn_labels;
mod state;
mod store;
mod update_check;
mod uploads;
mod usage_history;
mod webauthn;
mod webhook;
mod ws;

use std::path::PathBuf;
use std::sync::Arc;

use authz::Routes;
use axum::extract::DefaultBodyLimit;
use axum::http::Method;
use axum::routing::{any, get, post, put};
use axum::{Extension, Router, middleware};
use config::Config;
use live_sessions::live_sessions_predicate;
use registry::Registry;
use state::AppState;
use store::sessions::SessionRowStatus;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();
    let (config, pool, auth_config) = bootstrap().await?;
    let state = build_state(&config, pool, auth_config.clone()).await?;
    start_background_tasks(&state).await;
    let app = build_app(&state, &config, &auth_config);
    spawn_sweeps(state);
    serve(&config, app).await
}

fn init_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "cctui_server=info,sqlx::query=warn".into()),
        )
        .init();
}

async fn bootstrap() -> anyhow::Result<(Config, sqlx::PgPool, auth::AuthConfig)> {
    if let Err(e) = cctui_crypto::vault_key_checked() {
        anyhow::bail!("refusing to start: {e}");
    }

    let config = Config::from_env()?;
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
    Ok((config, pool, auth_config))
}

async fn build_state(
    config: &Config,
    pool: sqlx::PgPool,
    auth_config: auth::AuthConfig,
) -> anyhow::Result<AppState> {
    let skills = init_skill_store().await;
    let dispatchers = init_dispatchers(config);

    let presence = Arc::new(presence::PodIdentity::from_env());
    let http_client = reqwest::Client::new();

    let (transport, internal_secret) = init_bus(&pool, config, &presence, &http_client).await?;

    Ok(AppState {
        pool,
        config: config.clone(),
        registry: Registry::shared(),
        permission_store: routes::permissions::PermissionStore::shared(),
        // The single routing seam for daemon/dispatcher WS traffic;
        // the transport behind it is chosen by `init_bus`.
        bus: bus::Bus::new(transport),
        auth_config,
        // Passkeys ride the deployment's own public URL: a server already
        // configured with an https `CCTUI_EXTERNAL_URL` needs no new env.
        webauthn: webauthn::build(&config.external_url, config.rp_id.as_deref()).map(Arc::new),
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
        gateway_orphan_spam: Arc::new(dashmap::DashMap::new()),
        account_reauth: Arc::new(dashmap::DashMap::new()),
        codex_catalogs: Arc::new(dashmap::DashMap::new()),
        codex_account_catalogs: Arc::new(dashmap::DashMap::new()),
        codex_latest_version: Arc::new(std::sync::Mutex::new(None)),
        eviction_tracker: Arc::new(bandwidth_watch::EvictionTracker::default()),
        connect_tracker: Arc::new(bandwidth_watch::ConnectTracker::default()),
        divergence_tracker: Arc::new(bandwidth_watch::DivergenceTracker::default()),
        machine_event_inserts: Arc::new(dashmap::DashMap::new()),
        spawn_capabilities: Arc::new(dashmap::DashMap::new()),
        session_usd_budgets: Arc::new(dashmap::DashMap::new()),
        gateway_rate_windows: Arc::new(dashmap::DashMap::new()),
        update_check: update_check::UpdateCheck::shared(),
        self_update: Arc::new(routes::self_update::SelfUpdateGuard::default()),
        pending_commands: Arc::new(dashmap::DashMap::new()),
    })
}

/// Bus transport selection: with a routable pod IP this replica
/// participates in the peer mesh — mint/load the internal shared secret and
/// route/relay through `PeerHttpTransport`. Without one (local dev, single
/// replica) the bus stays local-only (`NoopTransport`) and writes nothing.
async fn init_bus(
    pool: &sqlx::PgPool,
    config: &Config,
    presence: &presence::PodIdentity,
    http_client: &reqwest::Client,
) -> anyhow::Result<(Box<dyn bus::Transport>, Option<Arc<str>>)> {
    let selected: (Box<dyn bus::Transport>, Option<Arc<str>>) = if presence.ip.is_some() {
        let secret = routes::internal::ensure_secret(pool).await?;
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
    Ok(selected)
}

async fn start_background_tasks(state: &AppState) {
    // Slow upstream release probe feeding `/version.latest_version`;
    // `CCTUI_UPDATE_CHECK=0` keeps air-gapped deployments quiet.
    if update_check::enabled_from_env() {
        tokio::spawn(update_check::task(state.update_check.clone(), state.http_client.clone()));
    }

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

    routes::codex_models::warm_cache(state).await;

    // Replica-aware WS presence: registered only when the pod knows
    // its routable IP; the heartbeat task keeps this pod's rows trusted and
    // reaps rows crashed pods left behind.
    if state.presence.ip.is_some() {
        tokio::spawn(presence::heartbeat_task(state.clone()));
    }
}

fn build_app(state: &AppState, config: &Config, auth_config: &auth::AuthConfig) -> Router {
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
        .layer(middleware::from_fn(auth::auth_middleware))
        .layer(Extension(auth_config.clone()));
    outer_routes()
        .nest("/api/v1", api_router)
        // Credentialed CORS bound to an explicit origin allowlist (same-origin
        // webui + dev Vite, extendable via CCTUI_ALLOWED_ORIGINS). A wildcard
        // origin is invalid once credentials are allowed.
        .layer(cors_layer(&config.allowed_origins))
        .with_state(state.clone())
}

// `{id}` etc. in route paths are axum path-param syntax, not format args.
#[allow(clippy::literal_string_with_formatting_args)]
fn outer_routes() -> Router<AppState> {
    Router::new()
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
        // Passkey login. Unauthenticated for the same reason `login` is: the
        // assertion IS the credential. `config` is the pre-auth probe the login
        // screen makes to decide whether to offer the passkey button at all.
        .route("/api/v1/auth/passkey/config", get(routes::passkeys::config))
        .route("/api/v1/auth/passkey/login/start", post(routes::passkeys::login_start))
        .route("/api/v1/auth/passkey/login/finish", post(routes::passkeys::login_finish))
        // Daemon-facing endpoints. `auth` and `ws` carry their own auth
        // (machine-key Bearer) so they live outside the user-token-only
        // `api_router` group.
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
        .route("/api/v1/daemon/sessions/{id}/limits", get(routes::session_limits::session_limits))
        .route("/api/v1/daemon/sessions/{id}/spawn-child", post(routes::spawn_child::spawn_child))
        .route(
            "/api/v1/daemon/sessions/{id}/message-child",
            post(routes::spawn_child::message_child),
        )
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
        // Update-hook endpoints. Machine-key Bearer self-auth like the blob
        // upload above, so they live outside the user-token `api_router`: a
        // daemon has no user token, and the health probe below is what tells
        // it whether the update it just ran actually took.
        .route("/api/v1/daemon/version", get(routes::update_hook::daemon_version))
        .route("/api/v1/daemon/update-hook/{run_id}", post(routes::update_hook::report))
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
        .route("/gateway/fireworks/{*path}", any(routes::gateway::fireworks))
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
}

fn spawn_sweeps(state: AppState) {
    spawn_periodic(REAPER_PERIOD, {
        let state = state.clone();
        move || webhook_sweep(state.clone())
    });
    spawn_periodic(REAPER_PERIOD, {
        let state = state.clone();
        move || keepalive_sweep(state.clone())
    });
    tokio::spawn(reaper_task(state));
}

async fn serve(config: &Config, app: Router) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(config.bind_addr()).await?;
    tracing::info!("listening on {}", config.bind_addr());
    axum::serve(listener, app).await?;
    Ok(())
}

/// Build the `/api/v1` route table from the descriptor list. Every
/// route declares both an [`authz::Authn`] (recorded; the proven `auth_middleware`
/// path still performs authentication) and an [`authz::Authz`] (enforced by
/// `authz::authz_layer`, default-deny for any un-policied route). Each route's
/// declared policy mirrors its CURRENT enforcement exactly: routes with
/// in-handler owner checks or `owner_filter()` SQL filters declare
/// `Authenticated` and keep that filter in the handler (the type system can't
/// express a self-scoped filter); scope-gated routes declare the matching
/// `Scope`. The returned [`Routes`] is the single source of truth walked by the
/// coverage test.
fn build_api_routes() -> Routes {
    api_routes::register(Routes::new())
}

/// Credentialed CORS layer restricted to `allowed_origins`. Credentials forbid
/// a wildcard origin, so the origin is an explicit list; request headers are
/// mirrored (not `*`) for the same reason.
fn cors_layer(allowed_origins: &[String]) -> tower_http::cors::CorsLayer {
    use axum::http::HeaderValue;
    use tower_http::cors::{AllowHeaders, AllowOrigin, CorsLayer};
    let origins: Vec<HeaderValue> =
        allowed_origins.iter().filter_map(|o| o.parse::<HeaderValue>().ok()).collect();
    CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_credentials(true)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers(AllowHeaders::mirror_request())
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

/// Auto-archive sessions silent past the TTL so the default list stays
/// self-cleaning, asking the daemon to remove each underlying job. `0` disables.
async fn auto_archive_stale(state: &AppState) {
    if state.config.archive_after_secs == 0 {
        return;
    }
    let cutoff = chrono::Utc::now()
        - chrono::Duration::seconds(
            i64::try_from(state.config.archive_after_secs).unwrap_or(i64::MAX),
        );
    match sqlx::query_scalar::<_, String>(
        // Drafts are staged-not-running — never auto-archive them.
        concat!(
            "UPDATE sessions SET status = 'archived', archived_by = 'automatic', \
                 ended_at = COALESCE(ended_at, now()), \
                 end_reason = COALESCE(end_reason, 'reaped_inactive') \
             WHERE ",
            live_sessions_predicate!(),
            " AND status <> ALL($2) \
               AND pinned = false AND last_heartbeat < $1 \
             RETURNING id"
        ),
    )
    .bind(cutoff)
    .bind(SessionRowStatus::names(SessionRowStatus::NOT_ARCHIVABLE))
    .fetch_all(&state.pool)
    .await
    {
        Ok(ids) if !ids.is_empty() => {
            tracing::info!(count = ids.len(), "auto-archived stale sessions");
            for id in &ids {
                crate::routes::sessions::dispatch_remove(
                    state,
                    id,
                    cctui_proto::adapter::RemoveInitiator::Automatic,
                )
                .await;
            }
        }
        Ok(_) => {}
        Err(err) => tracing::warn!(%err, "auto-archive sweep failed"),
    }
}

const REAPER_PERIOD: std::time::Duration = std::time::Duration::from_secs(30);

/// Runs `job` every `period` on its own task, so a slow pass delays only itself.
fn spawn_periodic<F, Fut>(period: std::time::Duration, mut job: F) -> tokio::task::JoinHandle<()>
where
    F: FnMut() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(period);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            job().await;
        }
    })
}

async fn webhook_sweep(state: AppState) {
    webhook::sweep(&state).await;
}

async fn keepalive_sweep(state: AppState) {
    keepalive::sweep(&state).await;
}

async fn reaper_task(state: AppState) {
    let mut interval = tokio::time::interval(REAPER_PERIOD);
    loop {
        interval.tick().await;
        let demoted = {
            let mut registry = state.registry.write().await;
            registry.mark_stale(state.config.inactive_after_secs)
        };
        if !demoted.is_empty() {
            let _ = sqlx::query("UPDATE sessions SET status = 'inactive' WHERE id = ANY($1)")
                .bind(&demoted)
                .execute(&state.pool)
                .await;
            tracing::info!(session_ids = ?demoted, "sessions demoted to inactive");
        }

        auto_archive_stale(&state).await;
        auto_archive::sweep(&state).await;
        spawn_labels::sweep(&state.pool).await;
        usage_history::sweep(&state);
        followup::sweep(&state.pool).await;

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

        auto_resume::sweep(&state).await;
        scheduled_messages::sweep(&state).await;

        state.permission_store.write().await.reap_stale(300); // seconds
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::{REAPER_PERIOD, spawn_periodic};

    #[tokio::test(start_paused = true)]
    async fn a_hung_periodic_job_does_not_stall_the_others() {
        let hung = spawn_periodic(REAPER_PERIOD, std::future::pending::<()>);
        let ticks = Arc::new(AtomicU32::new(0));
        let counter = ticks.clone();
        let reaper = spawn_periodic(REAPER_PERIOD, move || {
            counter.fetch_add(1, Ordering::SeqCst);
            async {}
        });
        tokio::time::sleep(REAPER_PERIOD * 10 + std::time::Duration::from_secs(1)).await;
        assert_eq!(ticks.load(Ordering::SeqCst), 11);
        hung.abort();
        reaper.abort();
    }

    #[test]
    fn api_route_table_is_unchanged() {
        let mut descs = super::build_api_routes().into_parts().1;
        descs.sort_by(|a, b| (a.path, a.method.as_str()).cmp(&(b.path, b.method.as_str())));
        let actual: Vec<String> = descs
            .iter()
            .map(|d| format!("{} {} {:?} {:?}", d.method, d.path, d.authn, d.authz))
            .collect();
        let expected = [
            "GET /account-pools Bearer Human",
            "POST /account-pools Bearer Human",
            "GET /account-pools/usage Bearer Human",
            "DELETE /account-pools/{id} Bearer Human",
            "PATCH /account-pools/{id} Bearer Human",
            "GET /accounts Bearer Human",
            "POST /accounts Bearer Human",
            "POST /accounts/oauth/finish Bearer Human",
            "POST /accounts/oauth/start Bearer Human",
            "GET /accounts/settings-catalog Bearer Authenticated",
            "GET /accounts/usage Bearer Human",
            "GET /accounts/usage/closes Bearer Human",
            "DELETE /accounts/{id} Bearer Human",
            "GET /accounts/{id} Bearer Human",
            "PATCH /accounts/{id} Bearer Human",
            "POST /accounts/{id}/limit-reset Bearer Human",
            "POST /accounts/{id}/providers Bearer Human",
            "DELETE /accounts/{id}/providers/{provider_id} Bearer Human",
            "PATCH /accounts/{id}/providers/{provider_id} Bearer Human",
            "POST /accounts/{id}/providers/{provider_id}/move Bearer Human",
            "PUT /accounts/{id}/redirect Bearer Human",
            "GET /accounts/{id}/shares Bearer Human",
            "POST /accounts/{id}/shares Bearer Human",
            "DELETE /accounts/{id}/shares/{user_id} Bearer Human",
            "GET /accounts/{id}/tool-policy Bearer Human",
            "PUT /accounts/{id}/tool-policy Bearer Human",
            "GET /accounts/{id}/usage Bearer Human",
            "GET /accounts/{id}/usage/closes Bearer Human",
            "GET /accounts/{id}/usage/history Bearer Human",
            "GET /admin/harness-autoupdate Bearer Scope(Admin)",
            "PUT /admin/harness-autoupdate Bearer Scope(Admin)",
            "PUT /admin/harness-autoupdate/{machine_id} Bearer Scope(Admin)",
            "PUT /admin/instance Bearer Scope(Admin)",
            "GET /admin/instance/self-update Bearer Scope(Admin)",
            "PUT /admin/instance/self-update Bearer Scope(Admin)",
            "DELETE /admin/machines/{id} Bearer Scope(Admin)",
            "PATCH /admin/machines/{id} Bearer Scope(Admin)",
            "DELETE /admin/machines/{id}/purge Bearer Scope(Admin)",
            "POST /admin/machines/{id}/rotate Bearer Scope(Admin)",
            "PUT /admin/passkeys/auto-prompt Bearer Scope(Admin)",
            "GET /admin/users Bearer Scope(Admin)",
            "POST /admin/users Bearer Scope(Admin)",
            "DELETE /admin/users/{id} Bearer Scope(Admin)",
            "PATCH /admin/users/{id} Bearer Scope(Admin)",
            "GET /admin/users/{id}/machines Bearer Scope(Admin)",
            "DELETE /admin/users/{id}/purge Bearer Scope(Admin)",
            "POST /admin/users/{id}/rotate Bearer Scope(Admin)",
            "GET /admin/users/{id}/tokens Bearer Scope(Admin)",
            "DELETE /admin/users/{id}/tokens/{token_id} Bearer Scope(Admin)",
            "PATCH /admin/users/{id}/tokens/{token_id} Bearer Scope(Admin)",
            "DELETE /admin/users/{id}/tokens/{token_id}/purge Bearer Scope(Admin)",
            "GET /bookmarks Bearer Authenticated",
            "POST /bookmarks Bearer Authenticated",
            "DELETE /bookmarks/{id} Bearer Authenticated",
            "PATCH /bookmarks/{id} Bearer Authenticated",
            "GET /capabilities Bearer Authenticated",
            "GET /daemon/binary/{target} Bearer Authenticated",
            "POST /deenroll Bearer Authenticated",
            "POST /dispatcher/enroll Bearer Scope(Enroll)",
            "GET /dispatchers Bearer Authenticated",
            "DELETE /dispatchers/{id} Bearer Scope(Enroll)",
            "PATCH /dispatchers/{id} Bearer Scope(Enroll)",
            "POST /enroll Bearer Scope(Enroll)",
            "GET /keys Bearer Authenticated",
            "POST /keys Bearer Authenticated",
            "DELETE /keys/{id} Bearer Authenticated",
            "GET /keys/{id}/value Bearer Authenticated",
            "GET /labels Bearer Authenticated",
            "POST /labels Bearer Authenticated",
            "DELETE /labels/{id} Bearer Authenticated",
            "PATCH /labels/{id} Bearer Authenticated",
            "GET /machines/resources Bearer Authenticated",
            r#"GET /machines/{machine_id}/codex-models Bearer Resource(Machine, Read, Path("machine_id"))"#,
            r#"POST /machines/{machine_id}/codex-models/refresh Bearer Resource(Machine, Read, Path("machine_id"))"#,
            "GET /machines/{machine_id}/commands/pending Bearer Authenticated",
            r#"GET /machines/{machine_id}/fs/dirs Bearer Resource(Machine, Read, Path("machine_id"))"#,
            r#"GET /machines/{machine_id}/fs/file Bearer Resource(Machine, Read, Path("machine_id"))"#,
            r#"GET /machines/{machine_id}/fs/gitinfo Bearer Resource(Machine, Read, Path("machine_id"))"#,
            r#"GET /machines/{machine_id}/status Bearer Resource(Machine, Read, Path("machine_id"))"#,
            "GET /manifest/daemon Bearer Authenticated",
            "GET /me Bearer Authenticated",
            "GET /models/codex Bearer Authenticated",
            "GET /passkeys Bearer Authenticated",
            "POST /passkeys/register/finish Bearer Authenticated",
            "POST /passkeys/register/start Bearer Authenticated",
            "POST /passkeys/test/finish Bearer Authenticated",
            "POST /passkeys/test/start Bearer Authenticated",
            "DELETE /passkeys/{id} Bearer Authenticated",
            "PATCH /passkeys/{id} Bearer Authenticated",
            "GET /permissions/pending Bearer Authenticated",
            "GET /profiles Bearer Human",
            "POST /profiles Bearer Human",
            "PUT /profiles/order Bearer Human",
            "DELETE /profiles/{id} Bearer Human",
            "PATCH /profiles/{id} Bearer Human",
            "GET /prompts Bearer Authenticated",
            "POST /prompts Bearer Authenticated",
            "GET /prompts/resolve Bearer Authenticated",
            "DELETE /prompts/{id} Bearer Authenticated",
            "GET /prompts/{id} Bearer Authenticated",
            "GET /redirects Bearer Human",
            "DELETE /redirects/{id} Bearer Human",
            "GET /sessions Bearer Authenticated",
            "POST /sessions/archive Bearer Authenticated",
            "POST /sessions/dispatch Bearer Scope(Dispatch)",
            "GET /sessions/dispatchers Bearer Authenticated",
            "POST /sessions/pin Bearer Authenticated",
            "GET /sessions/recent-dirs Bearer Authenticated",
            "POST /sessions/register Bearer Authenticated",
            "GET /sessions/search Bearer Authenticated",
            "GET /sessions/search/values Bearer Authenticated",
            "POST /sessions/spawn Bearer Authenticated",
            "GET /sessions/stats Bearer Authenticated",
            "GET /sessions/stats/cache-busts Bearer Authenticated",
            "GET /sessions/stats/tokens Bearer Authenticated",
            "GET /sessions/stats/usage Bearer Authenticated",
            "POST /sessions/unarchive Bearer Authenticated",
            "POST /sessions/unpin Bearer Authenticated",
            r#"GET /sessions/{id} Bearer Resource(Session, Read, Path("id"))"#,
            r#"PATCH /sessions/{id} Bearer Resource(Session, Write, Path("id"))"#,
            r#"POST /sessions/{id}/archive Bearer Resource(Session, Write, Path("id"))"#,
            r#"GET /sessions/{id}/attachments Bearer Resource(Session, Read, Path("id"))"#,
            r#"POST /sessions/{id}/auto-approve Bearer Resource(Session, Write, Path("id"))"#,
            r#"GET /sessions/{id}/bindings Bearer Resource(Session, Read, Path("id"))"#,
            r#"GET /sessions/{id}/blobs/{hash} Bearer Resource(Session, Read, Path("id"))"#,
            r#"GET /sessions/{id}/brief Bearer Resource(Session, Read, Path("id"))"#,
            r#"GET /sessions/{id}/conversation Bearer Resource(Session, Read, Path("id"))"#,
            r#"POST /sessions/{id}/deregister Bearer Resource(Session, Write, Path("id"))"#,
            r#"GET /sessions/{id}/diagnose Bearer Resource(Session, Read, Path("id"))"#,
            r#"POST /sessions/{id}/discard Bearer Resource(Session, Write, Path("id"))"#,
            r#"PUT /sessions/{id}/draft Bearer Resource(Session, Write, Path("id"))"#,
            r#"POST /sessions/{id}/files Bearer Resource(Session, Write, Path("id"))"#,
            r#"POST /sessions/{id}/fork Bearer Resource(Session, Write, Path("id"))"#,
            r#"GET /sessions/{id}/images/{image_id} Bearer Resource(Session, Read, Path("id"))"#,
            r#"POST /sessions/{id}/interrupt Bearer Resource(Session, Write, Path("id"))"#,
            r#"POST /sessions/{id}/keepalive Bearer Resource(Session, Write, Path("id"))"#,
            r#"POST /sessions/{id}/kill Bearer Resource(Session, Write, Path("id"))"#,
            r#"POST /sessions/{id}/labels Bearer Resource(Session, Write, Path("id"))"#,
            r#"DELETE /sessions/{id}/labels/{label_id} Bearer Resource(Session, Write, Path("id"))"#,
            r#"GET /sessions/{id}/langfuse Bearer Resource(Session, Read, Path("id"))"#,
            r#"POST /sessions/{id}/launch Bearer Resource(Session, Write, Path("id"))"#,
            r#"POST /sessions/{id}/message Bearer Resource(Session, Write, Path("id"))"#,
            r#"GET /sessions/{id}/messages/scheduled Bearer Resource(Session, Read, Path("id"))"#,
            r#"DELETE /sessions/{id}/messages/scheduled/{queue_id} Bearer Resource(Session, Write, Path("id"))"#,
            r#"PATCH /sessions/{id}/messages/scheduled/{queue_id} Bearer Resource(Session, Write, Path("id"))"#,
            r#"POST /sessions/{id}/messages/scheduled/{queue_id}/send-now Bearer Resource(Session, Write, Path("id"))"#,
            r#"POST /sessions/{id}/pin Bearer Resource(Session, Write, Path("id"))"#,
            r#"GET /sessions/{id}/pins Bearer Resource(Session, Read, Path("id"))"#,
            r#"POST /sessions/{id}/pins Bearer Resource(Session, Write, Path("id"))"#,
            r#"DELETE /sessions/{id}/pins/{seq} Bearer Resource(Session, Write, Path("id"))"#,
            r#"POST /sessions/{id}/policy Bearer Resource(Session, Write, Path("id"))"#,
            r#"GET /sessions/{id}/rebinds Bearer Resource(Session, Read, Path("id"))"#,
            r#"POST /sessions/{id}/resume Bearer Resource(Session, Write, Path("id"))"#,
            r#"POST /sessions/{id}/seen Bearer Resource(Session, Write, Path("id"))"#,
            r#"POST /sessions/{id}/set-model Bearer Resource(Session, Write, Path("id"))"#,
            r#"POST /sessions/{id}/switch-account Bearer Resource(Session, Write, Path("id"))"#,
            r#"POST /sessions/{id}/unarchive Bearer Resource(Session, Write, Path("id"))"#,
            r#"POST /sessions/{id}/unpin Bearer Resource(Session, Write, Path("id"))"#,
            "GET /settings Bearer Authenticated",
            "PUT /settings Bearer Authenticated",
            "POST /settings/rescrub Bearer Authenticated",
            "GET /skills/index Bearer Authenticated",
            "GET /skills/{name} Bearer Authenticated",
            "PUT /skills/{name} Bearer Authenticated",
            "GET /users/{id}/acls Bearer Scope(Admin)",
            "PATCH /users/{id}/acls Bearer Scope(Admin)",
            "GET /users/{id}/keys Bearer Scope(Admin)",
            "POST /users/{id}/keys Bearer Scope(Admin)",
            "DELETE /users/{id}/keys/{kid} Bearer Scope(Admin)",
            "PATCH /users/{id}/keys/{kid}/acls Bearer Scope(Admin)",
            "POST /users/{id}/tokens Bearer Authenticated",
            "GET /version Bearer Authenticated",
            "GET /version/changelog Bearer Authenticated",
            "POST /version/refresh Bearer Authenticated",
            "GET /version/self-update Bearer Authenticated",
            "POST /version/self-update Bearer Scope(Admin)",
            "GET /{resource_type}/{id}/shares Bearer Human",
            "POST /{resource_type}/{id}/shares Bearer Human",
            "DELETE /{resource_type}/{id}/shares/{user_id} Bearer Human",
        ];
        assert_eq!(actual, expected);
    }
}
