mod archive_store;
mod auth;
mod config;
mod crypto;
mod db;
mod policy;
mod registry;
mod routes;
mod skill_store;
mod state;
mod transcript_parser;
mod ws;

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, post, put};
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
    let auth_config =
        auth::AuthConfig::new(Config::admin_tokens(), Config::agent_tokens(), pool.clone());
    let (tui_tx, _) = tokio::sync::broadcast::channel(256);

    let archive = init_archive_store().await;
    let skills = init_skill_store().await;

    let state = AppState {
        pool,
        config: config.clone(),
        registry: Registry::shared(),
        channel_store: routes::channels::ChannelStore::shared(),
        permission_store: routes::permissions::PermissionStore::shared(),
        tui_tx,
        auth_config: auth_config.clone(),
        archive,
        skills,
    };

    let api_router = Router::new()
        .route("/sessions/register", post(routes::sessions::register))
        .route("/sessions/{id}/deregister", post(routes::sessions::deregister))
        .route("/sessions/spawn", post(routes::spawn::spawn_session))
        .route("/sessions", get(routes::admin::list_sessions))
        .route("/sessions/{id}", get(routes::admin::get_session))
        .route("/sessions/{id}/conversation", get(routes::admin::get_conversation))
        .route("/sessions/{id}/message", post(routes::admin::send_message))
        .route("/sessions/{id}/messages/pending", get(routes::admin::get_pending_messages))
        .route("/sessions/{id}/kill", post(routes::admin::kill_session))
        .route("/sessions/{id}/policy", post(routes::admin::set_session_policy))
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
        .route("/machines/{machine_id}/commands/pending", get(routes::spawn::get_machine_commands))
        .route("/enroll", post(routes::enroll::enroll))
        .route(
            "/admin/users",
            post(routes::admin_auth::create_user).get(routes::admin_auth::list_users),
        )
        .route("/admin/users/{id}", delete(routes::admin_auth::revoke_user))
        .route("/admin/users/{id}/rotate", post(routes::admin_auth::rotate_user))
        .route("/admin/users/{id}/machines", get(routes::admin_auth::list_user_machines))
        .route("/admin/machines/{id}", delete(routes::admin_auth::revoke_machine))
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
        .layer(middleware::from_fn(auth::auth_middleware))
        .layer(Extension(auth_config));

    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/install.sh", get(routes::install::install_sh))
        .route("/admin", get(routes::web::index))
        .route("/admin/", get(routes::web::index))
        .route("/api/v1/version", get(routes::web::version))
        .route("/api/v1/check", post(routes::check::check))
        .route("/api/v1/hooks/session-start", post(routes::channels::session_start_hook))
        .route("/api/v1/hooks/stop", post(routes::stop::stop))
        .route("/api/v1/hooks/post-tool-use", post(routes::post_tool_use::post_tool_use))
        .route("/api/v1/channels/register", post(routes::channels::register_channel))
        .route("/api/v1/channels/{channel_id}/session", get(routes::channels::poll_session))
        .route(
            "/api/v1/sessions/{session_id}/permission/request",
            post(routes::permissions::submit_request),
        )
        .route(
            "/api/v1/sessions/{session_id}/permission/decision/{request_id}",
            get(routes::permissions::poll_decision),
        )
        .route("/api/v1/events/{session_id}", post(routes::events::ingest))
        .route(
            "/api/v1/sessions/{session_id}/transcript",
            post(routes::transcript::ingest).get(routes::transcript::fetch),
        )
        .route("/api/v1/stream/{session_id}", get(ws::agent_stream))
        .route("/api/v1/ws", get(ws::tui_ws))
        .nest("/api/v1", api_router)
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

        {
            let mut store = state.channel_store.write().await;
            store.reap_stale(600); // 10 minutes
        }

        {
            let mut pstore = state.permission_store.write().await;
            pstore.reap_stale(300); // 5 minutes
        }
    }
}
