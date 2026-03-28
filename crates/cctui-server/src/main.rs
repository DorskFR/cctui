mod auth;
mod config;
mod crypto;
mod db;
mod policy;
mod registry;
mod routes;
mod state;
mod ws;

use axum::routing::{delete, get, post};
use axum::{Extension, Router, middleware};
use config::Config;
use registry::Registry;
use state::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "cctui_server=info".into()),
        )
        .init();

    let config = Config::from_env();
    let pool = db::connect(&config.database_url).await?;
    let auth_config = auth::AuthConfig {
        agent_tokens: Config::agent_tokens(),
        admin_tokens: Config::admin_tokens(),
    };
    let state = AppState {
        pool,
        config: config.clone(),
        registry: Registry::shared(),
        channel_store: routes::channels::ChannelStore::shared(),
        auth_config: auth_config.clone(),
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
        .layer(middleware::from_fn(auth::auth_middleware))
        .layer(Extension(auth_config));

    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/api/v1/check", post(routes::check::check))
        .route("/api/v1/channels/register", post(routes::channels::register_channel))
        .route("/api/v1/channels/{channel_id}/session", get(routes::channels::poll_session))
        .route("/api/v1/hooks/session-start", post(routes::channels::session_start_hook))
        .route("/api/v1/events/{session_id}", post(routes::events::ingest))
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

async fn reaper_task(state: AppState) {
    let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(30));
    loop {
        interval.tick().await;
        let terminated = {
            let mut registry = state.registry.write().await;
            registry.mark_stale(
                state.config.heartbeat_timeout_secs,
                state.config.terminated_timeout_secs,
            )
        };
        for session_id in &terminated {
            let _ = sqlx::query("UPDATE sessions SET status = 'terminated' WHERE id = $1")
                .bind(session_id.as_str())
                .execute(&state.pool)
                .await;
            tracing::info!(session_id = %session_id, "session terminated (stale)");
        }

        {
            let mut store = state.channel_store.write().await;
            store.reap_stale(600); // 10 minutes
        }
    }
}
