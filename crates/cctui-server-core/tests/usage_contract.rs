//! Headless quota reads preserve enforcement and the full host's reporting order.
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use cctui_server_core::gateway::credentials::CredentialContext;
use cctui_server_core::gateway::usage::{
    CachedUsage, USAGE_CACHE_TTL, UsageContext, fetch_account_usage, usage_for_soft_limit,
};
use dashmap::DashMap;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::sync::{Mutex, Notify};
use uuid::Uuid;

struct Host {
    pool: PgPool,
    client: reqwest::Client,
    locks: DashMap<Uuid, Arc<Mutex<()>>>,
    reauth: DashMap<Uuid, ()>,
    cache: DashMap<Uuid, CachedUsage>,
    url: String,
    reports: AtomicUsize,
    guard_calls: AtomicUsize,
    block_report: bool,
    entered: Notify,
    release: Notify,
}

impl CredentialContext for Host {
    fn pool(&self) -> &PgPool {
        &self.pool
    }
    fn http_client(&self) -> &reqwest::Client {
        &self.client
    }
    fn account_locks(&self) -> &DashMap<Uuid, Arc<Mutex<()>>> {
        &self.locks
    }
    fn account_reauth(&self) -> &DashMap<Uuid, ()> {
        &self.reauth
    }
    fn vault_key(&self) -> Vec<u8> {
        vec![42; 32]
    }
}

impl UsageContext for Host {
    fn usage_cache(&self) -> &DashMap<Uuid, CachedUsage> {
        &self.cache
    }
    fn probe_client(&self) -> &reqwest::Client {
        &self.client
    }
    fn probe_url_permitted(&self, _url: &str) -> Result<(), String> {
        self.guard_calls.fetch_add(1, Ordering::SeqCst);
        Err("denied by host policy".into())
    }
    fn anthropic_usage_url(&self) -> String {
        self.url.clone()
    }
    fn anthropic_reset_status_url(&self) -> String {
        self.url.replace("/usage", "/reset")
    }
    async fn usage_updated(&self, id: Uuid, _provider: String, usage: Option<&Value>) {
        {
            let cached = self.cache.get(&id).expect("cache precedes reporting");
            assert_eq!(cached.usage.as_ref(), usage);
            drop(cached);
        }
        self.reports.fetch_add(1, Ordering::SeqCst);
        if self.block_report {
            self.entered.notify_one();
            self.release.notified().await;
        }
    }
}

struct Fixture {
    host: Arc<Host>,
    provider: Uuid,
    calls: Arc<AtomicUsize>,
    server: tokio::task::JoinHandle<()>,
}

impl Fixture {
    async fn new(status: StatusCode, block_report: bool) -> Option<Self> {
        let url = ["DATABASE_URL", "TEST_DATABASE_URL"]
            .iter()
            .filter_map(|key| std::env::var(key).ok())
            .find(|value| !value.trim().is_empty());
        let Some(url) = url else {
            assert!(std::env::var_os("CI").is_none(), "quota contract needs a test database in CI");
            eprintln!("skipping quota contract: no test database");
            return None;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .after_connect(|conn, _| {
                Box::pin(async move {
                    sqlx::query("SET search_path = pg_temp").execute(conn).await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        sqlx::query("CREATE TEMP TABLE account_providers (LIKE public.account_providers INCLUDING DEFAULTS INCLUDING GENERATED)")
            .execute(&pool).await.unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/usage", listener.local_addr().unwrap());
        let app = axum::Router::new().route(
            "/usage",
            axum::routing::get({
                let calls = calls.clone();
                move || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    async move { (status, axum::Json(json!({"five_hour": {"utilization": 42.0}}))) }
                }
            }),
        );
        let app = app.route("/reset", axum::routing::get(|| async { axum::Json(json!({})) }));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let _ = rustls::crypto::ring::default_provider().install_default();
        let host = Arc::new(Host {
            pool,
            url,
            block_report,
            client: reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(2))
                .build()
                .unwrap(),
            locks: DashMap::new(),
            reauth: DashMap::new(),
            cache: DashMap::new(),
            reports: AtomicUsize::new(0),
            guard_calls: AtomicUsize::new(0),
            entered: Notify::new(),
            release: Notify::new(),
        });
        let provider = Uuid::new_v4();
        sqlx::query("INSERT INTO account_providers (id, user_id, account_id, provider, auth_scheme, encrypted_access_token)
            VALUES ($1, $2, $3, 'anthropic', 'bearer', $4)")
            .bind(provider).bind(Uuid::new_v4()).bind(Uuid::new_v4())
            .bind(cctui_crypto::encrypt("test-credential", &[42; 32]))
            .execute(&host.pool).await.unwrap();
        Some(Self { host, provider, calls, server })
    }

    async fn close(self) {
        self.server.abort();
        self.host.pool.close().await;
    }
}

#[tokio::test]
async fn cache_is_available_before_awaited_reporting_without_workspace_tables() {
    let Some(fixture) = Fixture::new(StatusCode::OK, true).await else { return };
    let request = {
        let host = fixture.host.clone();
        let provider = fixture.provider;
        tokio::spawn(async move { usage_for_soft_limit(host.as_ref(), provider).await })
    };
    tokio::time::timeout(Duration::from_secs(3), fixture.host.entered.notified())
        .await
        .expect("reporting must be invoked");
    assert!(!request.is_finished(), "the reading waits for its reporting hook");
    let hit = usage_for_soft_limit(fixture.host.as_ref(), fixture.provider).await.unwrap();
    assert_eq!(hit["five_hour"]["utilization"], 42.0);
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.host.reports.load(Ordering::SeqCst), 1);
    fixture.host.release.notify_one();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), request).await.unwrap().unwrap(),
        Some(hit)
    );
    fixture.close().await;
}

#[tokio::test]
async fn a_failed_quota_fetch_keeps_the_last_reading_without_reporting() {
    let Some(fixture) = Fixture::new(StatusCode::TOO_MANY_REQUESTS, false).await else { return };
    let previous = json!({"five_hour": {"utilization": 75.0}});
    let at = Instant::now()
        .checked_sub(USAGE_CACHE_TTL.to_std().unwrap() + Duration::from_secs(1))
        .unwrap();
    fixture
        .host
        .cache
        .insert(fixture.provider, CachedUsage { fetched_at: at, usage: Some(previous.clone()) });
    assert_eq!(usage_for_soft_limit(fixture.host.as_ref(), fixture.provider).await, Some(previous));
    assert_eq!(fixture.host.cache.get(&fixture.provider).unwrap().fetched_at, at);
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.host.reports.load(Ordering::SeqCst), 0);
    fixture.close().await;
}

#[tokio::test]
async fn a_probe_cannot_bypass_the_host_outbound_policy() {
    let Some(fixture) = Fixture::new(StatusCode::OK, false).await else { return };
    let probe = cctui_server_core::usage_probe::ids()[0];
    sqlx::query("UPDATE account_providers SET provider = 'openai-compatible', base_url = 'https://private.invalid', usage_probe = $2 WHERE id = $1")
        .bind(fixture.provider).bind(probe).execute(&fixture.host.pool).await.unwrap();
    assert_eq!(
        fetch_account_usage(fixture.host.as_ref(), fixture.provider).await,
        Err(StatusCode::BAD_GATEWAY)
    );
    assert_eq!(fixture.host.guard_calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.calls.load(Ordering::SeqCst), 0);
    fixture.close().await;
}
