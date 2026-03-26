mod auth;
mod config;
mod db;
mod registry;
mod routes;
mod state;
mod ws;

use axum::routing::{get, post};
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
        auth_config: auth_config.clone(),
    };

    let api_router = Router::new()
        .route("/sessions/register", post(routes::sessions::register))
        .route("/sessions/{id}/deregister", post(routes::sessions::deregister))
        .route("/sessions", get(routes::admin::list_sessions))
        .route("/sessions/{id}", get(routes::admin::get_session))
        .route("/sessions/{id}/conversation", get(routes::admin::get_conversation))
        .route("/sessions/{id}/message", post(routes::admin::send_message))
        .route("/sessions/{id}/kill", post(routes::admin::kill_session))
        .route("/bootstrap", get(routes::bootstrap::bootstrap))
        .route("/setup", get(routes::bootstrap::setup))
        .route("/check", post(routes::check::check))
        .layer(middleware::from_fn(auth::auth_middleware))
        .layer(Extension(auth_config));

    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/api/v1/stream/{session_id}", get(ws::agent_stream))
        .route("/api/v1/ws", get(ws::tui_ws))
        .nest("/api/v1", api_router)
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(config.bind_addr()).await?;
    tracing::info!("listening on {}", config.bind_addr());
    axum::serve(listener, app).await?;
    Ok(())
}
