//! Database-backed fixtures and isolated schema test helpers.

use sqlx::PgPool;
use uuid::Uuid;

#[must_use]
pub fn db_url(test_name: &str) -> Option<String> {
    let url = ["DATABASE_URL", "TEST_DATABASE_URL"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .find(|v| !v.trim().is_empty());

    if url.is_none() {
        assert!(
            std::env::var_os("CI").is_none(),
            "{test_name}: DATABASE_URL/TEST_DATABASE_URL must point at a migrated database in CI"
        );
        eprintln!("skipping {test_name}: no DATABASE_URL/TEST_DATABASE_URL");
    }
    url
}

/// Run a schema contract in an isolated database and clean it up after panics.
pub async fn with_isolated_database<F, Fut>(name: &str, test: F)
where
    F: FnOnce(sqlx::PgPool) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    use futures_util::FutureExt;
    use std::str::FromStr;

    let Some(url) = db_url(name) else { return };
    let options = sqlx::postgres::PgConnectOptions::from_str(&url).unwrap();
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect_with(options.clone())
        .await
        .unwrap();
    let database = format!("cctui_schema_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {database}")))
        .execute(&admin)
        .await
        .unwrap();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(6)
        .connect_with(options.database(&database))
        .await
        .unwrap();
    let result = std::panic::AssertUnwindSafe(test(pool.clone())).catch_unwind().await;
    pool.close().await;
    sqlx::query(sqlx::AssertSqlSafe(format!("DROP DATABASE {database} WITH (FORCE)")))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

/// Kernel state for domain tests, built like the host's `AppState::for_test`.
#[derive(Clone)]
pub struct TestHost {
    pub pool: sqlx::PgPool,
    pub bus: crate::bus::Bus,
    pub auth_config: crate::auth::AuthConfig,
    pub config: crate::config::Config,
    pub http_client: reqwest::Client,
    pub registry: crate::registry::SharedRegistry,
    pub pending_commands:
        std::sync::Arc<dashmap::DashMap<uuid::Uuid, crate::commands::PendingCommand>>,
    pub command_waiters: crate::commands::CommandWaiters,
    pub spawn_capabilities:
        std::sync::Arc<dashmap::DashMap<String, cctui_proto::api::SpawnCapability>>,
    pub session_usd_budgets: std::sync::Arc<dashmap::DashMap<String, f64>>,
}

impl TestHost {
    #[must_use]
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self {
            pool: pool.clone(),
            bus: crate::bus::Bus::new(Box::new(crate::bus::NoopTransport)),
            auth_config: crate::auth::AuthConfig::new(vec![], pool),
            config: crate::config::Config::for_test(vec![]),
            http_client: crate::client::build_http_client(),
            registry: crate::registry::Registry::shared(),
            pending_commands: std::sync::Arc::new(dashmap::DashMap::new()),
            command_waiters: std::sync::Arc::new(dashmap::DashMap::new()),
            spawn_capabilities: std::sync::Arc::new(dashmap::DashMap::new()),
            session_usd_budgets: std::sync::Arc::new(dashmap::DashMap::new()),
        }
    }
}

impl crate::context::KernelContext for TestHost {
    fn pool(&self) -> &sqlx::PgPool {
        &self.pool
    }
    fn bus(&self) -> &crate::bus::Bus {
        &self.bus
    }
    fn auth_config(&self) -> &crate::auth::AuthConfig {
        &self.auth_config
    }
    fn config(&self) -> &crate::config::Config {
        &self.config
    }
    fn http_client(&self) -> &reqwest::Client {
        &self.http_client
    }
    fn registry(&self) -> &crate::registry::SharedRegistry {
        &self.registry
    }
    fn pending_commands(&self) -> &dashmap::DashMap<uuid::Uuid, crate::commands::PendingCommand> {
        &self.pending_commands
    }
    fn command_waiters(&self) -> &crate::commands::CommandWaiters {
        &self.command_waiters
    }
    fn spawn_capabilities(&self) -> &dashmap::DashMap<String, cctui_proto::api::SpawnCapability> {
        &self.spawn_capabilities
    }
    fn session_usd_budgets(&self) -> &dashmap::DashMap<String, f64> {
        &self.session_usd_budgets
    }
}

pub async fn seed_session(pool: &PgPool, name: &str) -> (Uuid, String) {
    let uid = Uuid::new_v4();
    let machine = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
        .bind(uid)
        .bind(format!("{name}-{uid}"))
        .bind(format!("h-{uid}"))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, 'm', $3)")
        .bind(machine)
        .bind(uid)
        .bind(format!("mk-{machine}"))
        .execute(pool)
        .await
        .unwrap();
    let sid = format!("{name}-{uid}");
    sqlx::query(
        "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status, \
         adapter_id) VALUES ($1, $2, $2, $3, '/w', 'active', 'claude-code')",
    )
    .bind(&sid)
    .bind(machine)
    .bind(uid)
    .execute(pool)
    .await
    .unwrap();
    (uid, sid)
}
