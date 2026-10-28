//! Standalone automation server.
use anyhow::Context;
use cctui_core_server::host::Host;
use cctui_server_core::auth::AuthConfig;
use cctui_server_core::bus::{Bus, NoopTransport};
use cctui_server_core::state::CoreState;
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    cctui_server_core::client::install_crypto_provider();
    let database_url = std::env::var("DATABASE_URL").context("DATABASE_URL is required")?;
    let vault_key = cctui_crypto::vault_key_checked()?;
    let admin_tokens: Vec<String> = std::env::var("CCTUI_ADMIN_TOKENS")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    anyhow::ensure!(!admin_tokens.is_empty(), "CCTUI_ADMIN_TOKENS is required");
    let address = format!(
        "{}:{}",
        std::env::var("CCTUI_HOST").unwrap_or_else(|_| "0.0.0.0".into()),
        std::env::var("CCTUI_PORT").unwrap_or_else(|_| "8700".into())
    );
    let external_url = std::env::var("CCTUI_EXTERNAL_URL")
        .context("CCTUI_EXTERNAL_URL is required for worker gateway routing")?;
    let pool =
        sqlx::postgres::PgPoolOptions::new().max_connections(20).connect(&database_url).await?;
    sqlx::query("SELECT id FROM users LIMIT 0")
        .execute(&pool)
        .await
        .context("the automation beta currently requires an existing migrated schema")?;
    let auth = AuthConfig::new(admin_tokens, pool.clone());
    auth.seed_admin().await;
    let client = cctui_server_core::client::build_http_client();
    let mut dispatchers = cctui_server_core::dispatchers::Registry::new();
    if let Ok(raw) = std::env::var("CCTUI_HTTP_DISPATCHERS") {
        let entries: Vec<HttpDispatcher> =
            serde_json::from_str(&raw).context("invalid CCTUI_HTTP_DISPATCHERS")?;
        for entry in entries {
            dispatchers = dispatchers.with(Arc::new(
                cctui_server_core::dispatchers::http::HttpDispatcher::new(
                    &entry.id,
                    entry.url,
                    entry.token,
                    client.clone(),
                ),
            ));
        }
    }
    let spawn_defaults = cctui_proto::api::settings::SpawnDefaults {
        max_children: optional_number("CCTUI_SPAWN_MAX_CHILDREN")?,
        max_depth: optional_number("CCTUI_SPAWN_MAX_DEPTH")?,
        max_tree_budget_usd: optional_number("CCTUI_SPAWN_MAX_TREE_BUDGET_USD")?,
    };
    anyhow::ensure!(
        spawn_defaults.max_tree_budget_usd.is_none_or(|v| v.is_finite() && v >= 0.0),
        "CCTUI_SPAWN_MAX_TREE_BUDGET_USD must be finite and non-negative"
    );
    let host = Host::new(
        CoreState::new(
            pool,
            auth,
            Bus::new(Box::new(NoopTransport)),
            client,
            Arc::new(dispatchers),
        ),
        vault_key,
        external_url,
    )
    .with_spawn_defaults(spawn_defaults);
    let listener = tokio::net::TcpListener::bind(&address).await?;
    tracing::info!(%address, version = env!("CARGO_PKG_VERSION"), "automation server listening");
    let _background = cctui_core_server::background::BackgroundTasks::start(host.clone());
    axum::serve(listener, cctui_core_server::router::router(host))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
#[derive(serde::Deserialize)]
struct HttpDispatcher {
    id: String,
    url: String,
    #[serde(default)]
    token: Option<String>,
}

fn optional_number<T: std::str::FromStr>(name: &str) -> anyhow::Result<Option<T>> {
    match std::env::var(name) {
        Ok(raw) => raw.trim().parse().map(Some).map_err(|_| anyhow::anyhow!("invalid {name}")),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(error).with_context(|| format!("invalid {name}")),
    }
}
