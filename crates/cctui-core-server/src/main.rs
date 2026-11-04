//! Standalone automation server.
mod ownership;
use anyhow::Context;
use cctui_core_server::host::Host;
use cctui_server_core::auth::AuthConfig;
use cctui_server_core::bus::{Bus, NoopTransport};
use cctui_server_core::state::CoreState;
use std::future::IntoFuture;
use std::sync::Arc;

/// Env vars that may hold a secret ref instead of the value. `DATABASE_URL`
/// follows a rotation; the rest resolve once at startup.
const SECRET_ENV: [&str; 3] = ["CCTUI_ADMIN_TOKENS", "CCTUI_GITHUB_TOKEN", "GH_TOKEN"];

#[derive(clap::Parser)]
#[command(version)]
struct Args {
    /// Provision or validate the execution schema, then exit.
    #[arg(long)]
    init_db: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = <Args as clap::Parser>::parse();
    cctui_server_core::client::install_crypto_provider();
    let _telemetry = cctui_telemetry::init("cctui-core-server", env!("CARGO_PKG_VERSION"), "error");
    let registry = Arc::new(cctui_secrets::SecretRegistry::from_env());
    let raw_url = std::env::var("DATABASE_URL").context("DATABASE_URL is required")?;
    let database_url = registry.register("DATABASE_URL", &raw_url).await?.get();
    let mut secrets = std::collections::HashMap::new();
    for name in SECRET_ENV {
        if let Ok(raw) = std::env::var(name) {
            secrets.insert(name, registry.resolve(name, &raw).await?);
        }
    }
    let secret = |k: &str| secrets.get(k).cloned();
    if args.init_db {
        let pool = sqlx::postgres::PgPoolOptions::new().connect(&database_url).await?;
        cctui_server_core::schema::bootstrap(&pool).await?;
        return Ok(());
    }
    let vault_key = cctui_crypto::vault_key_checked()?;
    let admin_tokens: Vec<String> = secret("CCTUI_ADMIN_TOKENS")
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
    follow_database_url(&registry, &pool);
    let lease = ownership::RuntimeLease::acquire(&pool).await?;
    cctui_server_core::schema::bootstrap(&pool).await?;
    let auth = AuthConfig::new(admin_tokens, pool.clone());
    auth.seed_admin().await;
    let client = cctui_server_core::client::build_http_client();
    let dispatchers = http_dispatchers(&registry, &client).await?;
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
    .with_github_token(secret("CCTUI_GITHUB_TOKEN").or_else(|| secret("GH_TOKEN")))
    .with_spawn_defaults(spawn_defaults)
    .with_worker_ttl_secs(
        optional_number::<u64>("CCTUI_EPHEMERAL_MACHINE_TTL_HOURS")?
            .unwrap_or(2)
            .saturating_mul(60 * 60),
    );
    let listener = tokio::net::TcpListener::bind(&address).await?;
    tracing::info!(%address, version = env!("CARGO_PKG_VERSION"), "automation server listening");
    let _background = cctui_core_server::background::BackgroundTasks::start(host.clone());
    registry.spawn_reloader();
    let serving = axum::serve(listener, cctui_core_server::router::router(host))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .into_future();
    tokio::select! {
        result = serving => result?,
        result = lease.monitor() => result?,
    }
    Ok(())
}
#[derive(serde::Deserialize)]
struct HttpDispatcher {
    id: String,
    url: String,
    #[serde(default)]
    token: Option<String>,
}

/// `CCTUI_HTTP_DISPATCHERS`; each `token` may be a secret ref, resolved once.
async fn http_dispatchers(
    registry: &cctui_secrets::SecretRegistry,
    client: &reqwest::Client,
) -> anyhow::Result<cctui_server_core::dispatchers::Registry> {
    let mut dispatchers = cctui_server_core::dispatchers::Registry::new();
    let Ok(raw) = std::env::var("CCTUI_HTTP_DISPATCHERS") else { return Ok(dispatchers) };
    let entries: Vec<HttpDispatcher> =
        serde_json::from_str(&raw).context("invalid CCTUI_HTTP_DISPATCHERS")?;
    for entry in entries {
        let token = match entry.token {
            Some(raw) => {
                let name = format!("CCTUI_HTTP_DISPATCHERS[{}].token", entry.id);
                Some(registry.resolve(&name, &raw).await?)
            }
            None => None,
        };
        dispatchers =
            dispatchers.with(Arc::new(cctui_server_core::dispatchers::http::HttpDispatcher::new(
                &entry.id,
                entry.url,
                token,
                client.clone(),
            )));
    }
    Ok(dispatchers)
}

/// New pool connections use a rotated `DATABASE_URL`.
fn follow_database_url(registry: &cctui_secrets::SecretRegistry, pool: &sqlx::PgPool) {
    let Some(url) = registry.get("DATABASE_URL") else { return };
    let mut changes = url.subscribe();
    let pool = pool.clone();
    tokio::spawn(async move {
        while changes.changed().await.is_ok() {
            let url = changes.borrow_and_update().clone();
            match url.parse::<sqlx::postgres::PgConnectOptions>() {
                Ok(options) => {
                    pool.set_connect_options(options);
                    tracing::info!("DATABASE_URL rotated; new pool connections use it");
                }
                Err(_) => {
                    tracing::warn!("rotated DATABASE_URL does not parse; keeping the previous one");
                }
            }
        }
    });
}

fn optional_number<T: std::str::FromStr>(name: &str) -> anyhow::Result<Option<T>> {
    match std::env::var(name) {
        Ok(raw) => raw.trim().parse().map(Some).map_err(|_| anyhow::anyhow!("invalid {name}")),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(error).with_context(|| format!("invalid {name}")),
    }
}
