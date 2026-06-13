mod archive_store;
mod auth;
mod config;
mod crypto;
mod daemon_dispatch;
mod db;
mod dispatchers;
mod machine_liveness;
mod normalize;
mod ntfy;
mod policy;
mod registry;
mod routes;
mod skill_store;
mod state;
mod uploads;
mod ws;

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
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
    let auth_config = auth::AuthConfig::new(Config::admin_tokens(), pool.clone());
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
        dispatchers,
        pending_stage_requests: Arc::new(dashmap::DashMap::new()),
        pending_listdirs_requests: Arc::new(dashmap::DashMap::new()),
        machine_liveness: Arc::new(dashmap::DashMap::new()),
        account_locks: Arc::new(dashmap::DashMap::new()),
        http_client: reqwest::Client::new(),
        pending_oauth_logins: Arc::new(dashmap::DashMap::new()),
        account_usage_cache: Arc::new(dashmap::DashMap::new()),
    };

    let api_router = Router::new()
        .route("/sessions/register", post(routes::sessions::register))
        .route("/sessions/{id}/deregister", post(routes::sessions::deregister))
        .route(
            "/sessions/spawn",
            // Multipart spawn with file uploads (CCT-203): the route enforces a
            // 20 MB total cap itself; allow a little headroom over it for
            // multipart framing + base64 isn't applied until after parsing.
            post(routes::spawn::spawn_session).layer(DefaultBodyLimit::max(24 * 1024 * 1024)),
        )
        .route(
            // Mid-chat file attachments (CCT-236) — same multipart shape + caps
            // as spawn, same body-limit headroom.
            "/sessions/{id}/files",
            post(routes::spawn::stage_session_files).layer(DefaultBodyLimit::max(24 * 1024 * 1024)),
        )
        .route("/sessions/dispatch", post(routes::dispatch::dispatch))
        .route("/sessions/dispatchers", get(routes::dispatch::list_dispatchers))
        .route(
            "/dispatchers",
            get(routes::dispatchers::list_dispatchers).post(routes::dispatchers::create_dispatcher),
        )
        .route(
            "/dispatchers/{id}",
            patch(routes::dispatchers::update_dispatcher)
                .delete(routes::dispatchers::delete_dispatcher),
        )
        .route("/sessions/archive", post(routes::admin::archive_sessions))
        .route("/sessions/unarchive", post(routes::admin::unarchive_sessions))
        .route("/sessions/pin", post(routes::admin::pin_sessions))
        .route("/sessions/unpin", post(routes::admin::unpin_sessions))
        .route("/sessions", get(routes::admin::list_sessions))
        .route("/sessions/stats", get(routes::admin::session_stats))
        .route("/sessions/stats/tokens", get(routes::admin::session_token_stats))
        .route("/sessions/search", get(routes::admin::search_sessions))
        .route("/sessions/recent-dirs", get(routes::admin::recent_dirs))
        .route("/sessions/{id}", get(routes::admin::get_session))
        .route("/sessions/{id}", patch(routes::admin::rename_session))
        .route("/sessions/{id}/conversation", get(routes::admin::get_conversation))
        .route("/sessions/{id}/message", post(routes::admin::send_message))
        .route("/sessions/{id}/kill", post(routes::admin::kill_session))
        .route("/sessions/{id}/interrupt", post(routes::admin::interrupt_session))
        .route("/sessions/{id}/resume", post(routes::admin::resume_session))
        .route("/sessions/{id}/set-model", post(routes::admin::set_model))
        .route("/sessions/{id}/fork", post(routes::admin::fork_session))
        .route("/sessions/{id}/auto-approve", post(routes::admin::set_auto_approve))
        .route("/sessions/{id}/archive", post(routes::admin::archive_session))
        .route("/sessions/{id}/unarchive", post(routes::admin::unarchive_session))
        .route("/sessions/{id}/pin", post(routes::admin::pin_session))
        .route("/sessions/{id}/unpin", post(routes::admin::unpin_session))
        .route("/sessions/{id}/policy", post(routes::admin::set_session_policy))
        // Session labels (CCT-360): global label definitions + per-session
        // attach/detach.
        .route(
            "/labels",
            get(routes::admin::list_labels).post(routes::admin::create_label),
        )
        .route("/labels/{id}", axum::routing::delete(routes::admin::delete_label))
        .route("/sessions/{id}/labels", post(routes::admin::attach_label))
        .route(
            "/sessions/{id}/labels/{label_id}",
            axum::routing::delete(routes::admin::detach_label),
        )
        .route("/manifest/daemon", get(routes::manifest::daemon_manifest))
        .route("/daemon/binary/{target}", get(routes::manifest::download_daemon_binary))
        .route("/prompts", get(routes::prompts::list_prompts).post(routes::prompts::create_prompt))
        .route(
            "/prompts/{id}",
            get(routes::prompts::get_prompt).delete(routes::prompts::delete_prompt),
        )
        .route(
            "/keys",
            get(routes::credentials::list_api_keys).post(routes::credentials::create_api_key),
        )
        .route("/keys/{id}", delete(routes::credentials::delete_api_key))
        .route("/keys/{id}/value", get(routes::credentials::get_api_key_value))
        .route(
            "/accounts",
            get(routes::accounts::list_accounts).post(routes::accounts::create_account),
        )
        .route("/accounts/oauth/start", post(routes::accounts::oauth_start))
        .route("/accounts/oauth/finish", post(routes::accounts::oauth_finish))
        .route(
            "/accounts/{id}",
            patch(routes::accounts::rename_account).delete(routes::accounts::delete_account),
        )
        .route("/accounts/{id}/usage", get(routes::accounts::account_usage))
        .route("/machines/{machine_id}/commands/pending", get(routes::spawn::get_machine_commands))
        .route("/machines/{machine_id}/fs/dirs", get(routes::fs::list_dirs))
        .route("/me", get(routes::me::me))
        .route("/enroll", post(routes::enroll::enroll))
        .route("/deenroll", post(routes::enroll::deenroll))
        .route(
            "/admin/users",
            post(routes::admin_auth::create_user).get(routes::admin_auth::list_users),
        )
        .route(
            "/admin/users/{id}",
            delete(routes::admin_auth::revoke_user).patch(routes::admin_auth::update_user),
        )
        .route("/admin/users/{id}/purge", delete(routes::admin_auth::purge_user))
        .route("/admin/users/{id}/rotate", post(routes::admin_auth::rotate_user))
        .route("/admin/users/{id}/machines", get(routes::admin_auth::list_user_machines))
        .route("/admin/users/{id}/tokens", get(routes::admin_auth::list_user_tokens))
        .route(
            "/admin/users/{id}/tokens/{token_id}",
            patch(routes::admin_auth::relabel_user_token)
                .delete(routes::admin_auth::revoke_user_token),
        )
        .route(
            "/admin/users/{id}/tokens/{token_id}/purge",
            delete(routes::admin_auth::delete_user_token),
        )
        .route(
            "/admin/machines/{id}",
            delete(routes::admin_auth::revoke_machine).patch(routes::admin_auth::rename_machine),
        )
        .route("/admin/machines/{id}/rotate", post(routes::admin_auth::rotate_machine))
        .route("/admin/machines/{id}/purge", delete(routes::admin_auth::delete_machine))
        .route("/archive/index", get(routes::archive::index))
        .route("/archive/status", get(routes::archive::get_status))
        .route(
            "/archive/manifest",
            post(routes::archive::post_manifest).layer(DefaultBodyLimit::max(8 * 1024 * 1024)),
        )
        .route(
            "/archive/{project_dir}/{session_id}",
            put(routes::archive::put)
                .head(routes::archive::head)
                .get(routes::archive::get)
                .layer(DefaultBodyLimit::max(100 * 1024 * 1024)),
        )
        .route("/permissions/pending", get(routes::permissions::list_pending))
        .route("/skills/index", get(routes::skills::index))
        .route(
            "/skills/{name}",
            put(routes::skills::put)
                .get(routes::skills::get)
                .layer(DefaultBodyLimit::max(50 * 1024 * 1024)),
        )
        .route("/users/{id}/tokens", post(routes::daemon::mint_user_token))
        .layer(middleware::from_fn(auth::auth_middleware))
        .layer(Extension(auth_config));

    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/admin", get(routes::web::index))
        .route("/admin/", get(routes::web::index))
        .route("/api/v1/version", get(routes::web::version))
        .route("/api/v1/ws", get(ws::tui_ws))
        // Daemon-facing endpoints. `auth` and `ws` carry their own auth
        // (machine-key Bearer / `?token=` query) so they live outside the
        // user-token-only `api_router` group.
        .route("/api/v1/daemon/auth", post(routes::daemon::auth))
        .route("/api/v1/daemon/ws", get(routes::daemon::ws))
        .route("/api/v1/triggers/{kind}", post(routes::triggers::ingest))
        // OAuth passthrough gateway (CCT-232). Auths via the session-scoped
        // token in the request's own Authorization header — NOT the user-token
        // `api_router` middleware — so it lives on the outer app. Matches any
        // method + sub-path under each provider prefix.
        .route("/gateway/anthropic/{*path}", any(routes::gateway::anthropic))
        .route("/gateway/openai/{*path}", any(routes::gateway::openai))
        .nest("/api/v1", api_router)
        // The standalone web UI is served from a different origin and talks to
        // this API cross-origin. Auth is via the `Authorization` Bearer header
        // (no cookies), so a credential-less permissive policy is safe and
        // avoids per-origin config. WS upgrades auth via `?token=` and are not
        // subject to CORS preflight.
        .layer(tower_http::cors::CorsLayer::permissive())
        .with_state(state.clone());

    tokio::spawn(reaper_task(state));

    let listener = tokio::net::TcpListener::bind(config.bind_addr()).await?;
    tracing::info!("listening on {}", config.bind_addr());
    axum::serve(listener, app).await?;
    Ok(())
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

/// Construct the [`dispatchers::Registry`] from config (CCT-234).
///
/// Merges the legacy `CCTUI_HTTP_DISPATCHERS` (http-only, back-compat) with the
/// kind-tagged `CCTUI_DISPATCHERS`. Native kube/docker dispatchers do a connect
/// probe at startup: if the backend is unreachable (e.g. no docker socket in the
/// k8s pod, or no kube config off-cluster) the instance is logged and skipped
/// rather than aborting boot — the rest of the registry still comes up.
#[allow(clippy::cognitive_complexity)]
async fn init_dispatchers(config: &Config) -> Arc<dispatchers::Registry> {
    use config::DispatcherConfig;

    let mut registry = dispatchers::Registry::new();

    for d in &config.http_dispatchers {
        tracing::info!(id = %d.id, url = %d.url, "http dispatcher registered (legacy CCTUI_HTTP_DISPATCHERS)");
        registry = registry.with(Arc::new(dispatchers::http::HttpDispatcher::new(
            &d.id,
            &d.url,
            d.token.clone(),
        )));
    }

    for d in &config.dispatchers {
        match d {
            DispatcherConfig::Http(c) => {
                tracing::info!(id = %c.id, url = %c.url, "http dispatcher registered");
                registry = registry.with(Arc::new(dispatchers::http::HttpDispatcher::new(
                    &c.id,
                    &c.url,
                    c.token.clone(),
                )));
            }
            DispatcherConfig::Kube(c) => {
                match dispatchers::kube::KubeDispatcher::try_new(c).await {
                    Ok(k) => {
                        tracing::info!(id = %c.id, namespace = %c.namespace, source_cronjob = %c.source_cronjob, "kube dispatcher registered");
                        registry = registry.with(Arc::new(k));
                    }
                    Err(e) => {
                        tracing::warn!(id = %c.id, "kube dispatcher skipped (no kube client): {e}")
                    }
                }
            }
            DispatcherConfig::Docker(c) => {
                match dispatchers::docker::DockerDispatcher::try_new(c).await {
                    Ok(k) => {
                        tracing::info!(id = %c.id, image = %c.image, "docker dispatcher registered");
                        registry = registry.with(Arc::new(k));
                    }
                    Err(e) => {
                        tracing::warn!(id = %c.id, "docker dispatcher skipped (no docker socket): {e}")
                    }
                }
            }
        }
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
                "UPDATE sessions SET status = 'archived' \
                 WHERE status != 'archived' AND pinned = false AND last_heartbeat < $1",
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

        // Machine liveness (CCT-255): re-derive every machine's tier from its
        // `last_seen_at` and broadcast any transitions. The 30s cadence means a
        // daemon that stops heartbeating ages online → stale → offline on its
        // own — the acceptance case "killing a daemon flips it offline within
        // one liveness window without a dispatch attempt".
        machine_liveness::sweep(&state).await;

        {
            let mut pstore = state.permission_store.write().await;
            pstore.reap_stale(300); // 5 minutes
        }
    }
}
