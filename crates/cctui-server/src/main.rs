mod config;
mod db;

use config::Config;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "cctui_server=info".into()),
        )
        .init();

    let config = Config::from_env();
    let _pool = db::connect(&config.database_url).await?;

    let app = axum::Router::new().route("/health", axum::routing::get(|| async { "ok" }));

    let listener = tokio::net::TcpListener::bind(config.bind_addr()).await?;
    tracing::info!("listening on {}", config.bind_addr());
    axum::serve(listener, app).await?;

    Ok(())
}
