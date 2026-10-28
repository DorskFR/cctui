//! Credential minting works with execution tables and a headless host.
use std::collections::BTreeMap;

use cctui_server_core::gateway::Family;
use cctui_server_core::gateway::mint::{MintContext, mint_env_for_account, resume_env_for_session};
use dashmap::DashMap;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

struct Host {
    pool: PgPool,
    cleared: DashMap<String, ()>,
    client: reqwest::Client,
    locks: DashMap<Uuid, std::sync::Arc<tokio::sync::Mutex<()>>>,
    reauth: DashMap<Uuid, ()>,
    cache: DashMap<Uuid, cctui_server_core::gateway::usage::CachedUsage>,
    caps: DashMap<String, cctui_proto::api::SpawnCapability>,
    budgets: DashMap<String, f64>,
    reject_rebind: std::sync::atomic::AtomicBool,
}

impl MintContext for Host {
    fn pool(&self) -> &PgPool {
        &self.pool
    }
    fn vault_key(&self) -> Vec<u8> {
        vec![42; 32]
    }
    fn external_url(&self) -> &'static str {
        "https://automation.example.invalid/"
    }
    fn clear_orphan_fingerprint(&self, fingerprint: &str) {
        self.cleared.insert(fingerprint.to_owned(), ());
    }
}

struct Fixture {
    host: Host,
    owner: Uuid,
    grantee: Uuid,
    account: Uuid,
    anthropic: Uuid,
    openai: Uuid,
}

impl Fixture {
    async fn new() -> Option<Self> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let url = ["DATABASE_URL", "TEST_DATABASE_URL"]
            .iter()
            .filter_map(|key| std::env::var(key).ok())
            .find(|value| !value.trim().is_empty());
        let Some(url) = url else {
            assert!(
                std::env::var_os("CI").is_none(),
                "mint contract requires a test database in CI"
            );
            eprintln!("skipping mint contract: no test database");
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
        for ddl in [
            "CREATE TEMP TABLE accounts (LIKE public.accounts INCLUDING DEFAULTS INCLUDING GENERATED)",
            "CREATE TEMP TABLE account_providers (LIKE public.account_providers INCLUDING DEFAULTS INCLUDING GENERATED)",
            "CREATE TEMP TABLE session_tokens (LIKE public.session_tokens INCLUDING DEFAULTS INCLUDING GENERATED)",
            "CREATE TEMP TABLE sessions (LIKE public.sessions INCLUDING DEFAULTS INCLUDING GENERATED)",
            "CREATE TEMP TABLE resource_shares (LIKE public.resource_shares INCLUDING DEFAULTS INCLUDING GENERATED)",
            "CREATE TEMP TABLE account_pools (LIKE public.account_pools INCLUDING DEFAULTS)",
            "CREATE TEMP TABLE session_token_usage (session_id text)",
            "CREATE TEMP TABLE session_spawn_capabilities (session_id text PRIMARY KEY, capability jsonb)",
            "CREATE TEMP TABLE auth_keys (LIKE public.auth_keys INCLUDING DEFAULTS INCLUDING GENERATED)",
        ] {
            sqlx::query(ddl).execute(&pool).await.unwrap();
        }
        let owner = Uuid::new_v4();
        let grantee = Uuid::new_v4();
        let account: Uuid = sqlx::query_scalar("INSERT INTO accounts (user_id, name, env_json) VALUES ($1, 'automation', $2) RETURNING id")
            .bind(owner)
            .bind(cctui_crypto::encrypt(&json!({"ENABLE_PROMPT_CACHING_1H":"0", "LD_PRELOAD":"/tmp/untrusted.so", "OPENAI_BASE_URL":"https://untrusted.invalid"}).to_string(), &[42; 32]))
            .fetch_one(&pool).await.unwrap();
        let mut providers = Vec::new();
        for provider in ["anthropic", "openai"] {
            let id: Uuid = sqlx::query_scalar("INSERT INTO account_providers (user_id, account_id, provider, settings_json) VALUES ($1, $2, $3, $4) RETURNING id")
                .bind(owner).bind(account).bind(provider)
                .bind(json!({"model_verbosity":"low", "web_search":"enabled"}))
                .fetch_one(&pool).await.unwrap();
            providers.push(id);
        }
        assert!(
            sqlx::query_scalar::<_, Option<String>>(
                "SELECT to_regclass('session_attachments')::text"
            )
            .fetch_one(&pool)
            .await
            .unwrap()
            .is_none()
        );
        Some(Self {
            host: Host {
                pool,
                cleared: DashMap::new(),
                client: reqwest::Client::new(),
                locks: DashMap::new(),
                reauth: DashMap::new(),
                cache: DashMap::new(),
                caps: DashMap::new(),
                budgets: DashMap::new(),
                reject_rebind: std::sync::atomic::AtomicBool::new(false),
            },
            owner,
            grantee,
            account,
            anthropic: providers[0],
            openai: providers[1],
        })
    }

    async fn mint(
        &self,
        provider: Uuid,
        family: Family,
        session: &str,
        user: Uuid,
    ) -> BTreeMap<String, String> {
        mint_env_for_account(&self.host, provider, family.label(), session, Some(user))
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn stable_family_tokens_survive_remint_and_rotate_after_revocation() {
    let Some(f) = Fixture::new().await else { return };
    let session = Uuid::new_v4().to_string();
    let anthropic = f.mint(f.anthropic, Family::Anthropic, &session, f.owner).await;
    let openai = f.mint(f.openai, Family::Openai, &session, f.owner).await;
    assert_eq!(
        anthropic["ANTHROPIC_BASE_URL"],
        "https://automation.example.invalid/gateway/anthropic"
    );
    assert_eq!(openai["OPENAI_BASE_URL"], "https://automation.example.invalid/gateway/openai");
    assert_eq!(anthropic["ENABLE_PROMPT_CACHING_1H"], "0");
    assert_eq!(anthropic["LD_PRELOAD"], "/tmp/untrusted.so");
    assert!(openai[cctui_proto::codex_config::CONFIG_TOML_ENV].contains("web_search"));
    let again = f.mint(f.anthropic, Family::Anthropic, &session, f.owner).await;
    assert_eq!(again["ANTHROPIC_AUTH_TOKEN"], anthropic["ANTHROPIC_AUTH_TOKEN"]);
    assert!(
        f.host
            .cleared
            .contains_key(&cctui_server_core::auth::sha256_hex(&again["ANTHROPIC_AUTH_TOKEN"]))
    );
    let resumed = resume_env_for_session(&f.host, &session).await;
    assert_eq!(resumed["ANTHROPIC_AUTH_TOKEN"], anthropic["ANTHROPIC_AUTH_TOKEN"]);
    assert_eq!(resumed["OPENAI_API_KEY"], openai["OPENAI_API_KEY"]);
    cctui_server_core::store::tokens::revoke_by_session(&f.host.pool, &session).await.unwrap();
    let resumed = resume_env_for_session(&f.host, &session).await;
    assert_ne!(resumed["ANTHROPIC_AUTH_TOKEN"], anthropic["ANTHROPIC_AUTH_TOKEN"]);
    assert_ne!(resumed["OPENAI_API_KEY"], openai["OPENAI_API_KEY"]);
    let old_revoked: bool = sqlx::query_scalar(
        "SELECT revoked_at IS NOT NULL FROM session_tokens WHERE token_hash = $1",
    )
    .bind(cctui_server_core::auth::sha256_hex(&anthropic["ANTHROPIC_AUTH_TOKEN"]))
    .fetch_one(&f.host.pool)
    .await
    .unwrap();
    assert!(
        old_revoked,
        "resuming must create a fresh credential without reviving a revoked token"
    );
}

#[tokio::test]
async fn grantee_mints_filter_settings_and_env_and_revocation_blocks_resume() {
    let Some(f) = Fixture::new().await else { return };
    sqlx::query("INSERT INTO resource_shares (resource_type, resource_id, grantee_id) VALUES ('account', $1, $2)")
        .bind(f.account).bind(f.grantee).execute(&f.host.pool).await.unwrap();
    let session = Uuid::new_v4().to_string();
    let env = f.mint(f.openai, Family::Openai, &session, f.grantee).await;
    assert_eq!(env["OPENAI_BASE_URL"], "https://automation.example.invalid/gateway/openai");
    assert_eq!(env["ENABLE_PROMPT_CACHING_1H"], "0");
    assert!(!env.contains_key("LD_PRELOAD"));
    let config = &env[cctui_proto::codex_config::CONFIG_TOML_ENV];
    assert!(config.contains("model_verbosity"));
    assert!(!config.contains("web_search"));
    sqlx::query("UPDATE resource_shares SET revoked_at = now() WHERE resource_id = $1")
        .bind(f.account)
        .execute(&f.host.pool)
        .await
        .unwrap();
    assert!(resume_env_for_session(&f.host, &session).await.is_empty());
    assert!(f.mint(f.openai, Family::Openai, &session, f.grantee).await.is_empty());
    let revoked: bool = sqlx::query_scalar(
        "SELECT revoked_at IS NOT NULL FROM session_tokens WHERE token_hash = $1",
    )
    .bind(cctui_server_core::auth::sha256_hex(&env["OPENAI_API_KEY"]))
    .fetch_one(&f.host.pool)
    .await
    .unwrap();
    assert!(revoked);
}

impl cctui_server_core::gateway::credentials::CredentialContext for Host {
    fn pool(&self) -> &PgPool {
        &self.pool
    }
    fn http_client(&self) -> &reqwest::Client {
        &self.client
    }
    fn account_locks(&self) -> &DashMap<Uuid, std::sync::Arc<tokio::sync::Mutex<()>>> {
        &self.locks
    }
    fn account_reauth(&self) -> &DashMap<Uuid, ()> {
        &self.reauth
    }
    fn vault_key(&self) -> Vec<u8> {
        vec![42; 32]
    }
}
impl cctui_server_core::gateway::usage::UsageContext for Host {
    fn usage_cache(&self) -> &DashMap<Uuid, cctui_server_core::gateway::usage::CachedUsage> {
        &self.cache
    }
    fn probe_client(&self) -> &reqwest::Client {
        &self.client
    }
    fn probe_url_permitted(&self, _: &str) -> Result<(), String> {
        Err("no outbound probes in this contract".into())
    }
    async fn usage_updated(&self, _: Uuid, _: String, _: Option<&serde_json::Value>) {}
}
impl cctui_server_core::account_resolve::AccountResolutionContext for Host {
    fn pool(&self) -> &PgPool {
        &self.pool
    }
    fn cached_needs_reauth(&self, id: Uuid) -> bool {
        self.reauth.contains_key(&id)
    }
    async fn usage_for_soft_limit(&self, id: Uuid) -> Option<serde_json::Value> {
        cctui_server_core::gateway::usage::usage_for_soft_limit(self, id).await
    }
}
impl cctui_server_core::session_credentials::SessionCredentialContext for Host {
    fn token_pool(&self) -> &PgPool {
        &self.pool
    }
    fn spawn_capabilities(&self) -> &DashMap<String, cctui_proto::api::SpawnCapability> {
        &self.caps
    }
    fn session_budgets(&self) -> &DashMap<String, f64> {
        &self.budgets
    }
    async fn after_execution_rebind(&self, spawn: &str, session: &str) -> Result<(), sqlx::Error> {
        let id: String =
            sqlx::query_scalar("SELECT session_id FROM session_tokens WHERE session_id = $1")
                .bind(session)
                .fetch_one(&self.pool)
                .await?;
        assert_eq!(id, session, "execution records precede the host callback");
        assert!(self.budgets.contains_key(spawn), "cache follows the host callback");
        if self.reject_rebind.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(sqlx::Error::RowNotFound);
        }
        Ok(())
    }
}

#[tokio::test]
async fn machine_account_preparation_needs_no_workspace_storage() {
    use cctui_server_core::machine_spawn::{AccountTarget, resolve_spawn_account};
    let Some(f) = Fixture::new().await else { return };
    let target = AccountTarget {
        uid: f.owner,
        adapter_id: "claude-code".into(),
        token_session_id: "spawn".into(),
    };
    let mut req: cctui_proto::api::SpawnRequest = serde_json::from_value(json!({"machine_id":"machine", "working_dir":"/work", "env":{"CUSTOM":"kept"}, "effort":"  "})).unwrap();
    let bound = resolve_spawn_account(&f.host, &target, &req).await.unwrap();
    assert_eq!(bound.account_choice.as_deref(), Some("automation"));
    assert!(bound.effort.is_none());
    assert_eq!(bound.env["CUSTOM"], "kept");
    assert!(bound.env.contains_key("ANTHROPIC_AUTH_TOKEN"));
    assert!(bound.env.contains_key("OPENAI_API_KEY"));
    req.no_account = true;
    let unbound = resolve_spawn_account(&f.host, &target, &req).await.unwrap();
    assert!(unbound.account_choice.is_none());
    assert_eq!(unbound.env, req.env);
    req.account = Some("automation".into());
    let explicit = resolve_spawn_account(&f.host, &target, &req).await.unwrap();
    assert_eq!(explicit.account_choice.as_deref(), Some("automation"));
    assert_eq!(explicit.env["ANTHROPIC_AUTH_TOKEN"], bound.env["ANTHROPIC_AUTH_TOKEN"]);
    sqlx::query("INSERT INTO accounts (user_id, name) VALUES ($1, 'second')")
        .bind(f.owner)
        .execute(&f.host.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO account_providers (user_id, account_id, provider) SELECT $1, id, 'anthropic' FROM accounts WHERE name = 'second'").bind(f.owner).execute(&f.host.pool).await.unwrap();
    req.account = None;
    req.no_account = false;
    let (status, error) = resolve_spawn_account(&f.host, &target, &req).await.err().unwrap();
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
    assert!(error.error.contains("several are available"));
}

#[tokio::test]
async fn credential_rebind_and_revocation_preserve_host_order_without_attachments() {
    use cctui_proto::ids::{SessionId, SpawnKey};
    use cctui_server_core::session_credentials::{rebind_spawn_key, revoke_session_tokens};
    let Some(f) = Fixture::new().await else { return };
    f.mint(f.anthropic, Family::Anthropic, "spawn", f.owner).await;
    f.host.budgets.insert("spawn".into(), 12.0);
    let cap: cctui_proto::api::SpawnCapability =
        serde_json::from_value(json!({"adapters":["claude-code"],"max_children":2})).unwrap();
    cctui_server_core::store::spawn_capabilities::upsert(&f.host.pool, "spawn", &cap)
        .await
        .unwrap();
    f.host.caps.insert("spawn".into(), cap);
    sqlx::query("INSERT INTO session_token_usage VALUES ('spawn')")
        .execute(&f.host.pool)
        .await
        .unwrap();
    f.host.reject_rebind.store(true, std::sync::atomic::Ordering::Relaxed);
    rebind_spawn_key(&f.host, SpawnKey::from("spawn"), SessionId::from("registered")).await;
    assert!(f.host.budgets.contains_key("spawn"), "failed host callback must stop cache changes");
    f.host.reject_rebind.store(false, std::sync::atomic::Ordering::Relaxed);
    rebind_spawn_key(&f.host, SpawnKey::from("spawn"), SessionId::from("registered")).await;
    assert!(!f.host.budgets.contains_key("spawn"));
    assert!((*f.host.budgets.get("registered").unwrap() - 12.0).abs() < f64::EPSILON);
    assert!(!f.host.caps.contains_key("spawn"));
    assert_eq!(f.host.caps.get("registered").unwrap().max_children, Some(2));
    assert_eq!(
        cctui_server_core::store::spawn_capabilities::get(&f.host.pool, "registered")
            .await
            .unwrap()
            .unwrap()
            .max_children,
        Some(2)
    );
    let id: String = sqlx::query_scalar("SELECT session_id FROM session_token_usage")
        .fetch_one(&f.host.pool)
        .await
        .unwrap();
    assert_eq!(id, "registered");
    revoke_session_tokens(&f.host, "registered").await;
    assert!(f.host.budgets.is_empty());
    assert!(f.host.caps.is_empty());
    assert!(
        cctui_server_core::store::spawn_capabilities::get(&f.host.pool, "registered")
            .await
            .unwrap()
            .is_none()
    );
    let revoked: bool = sqlx::query_scalar("SELECT revoked_at IS NOT NULL FROM session_tokens")
        .fetch_one(&f.host.pool)
        .await
        .unwrap();
    assert!(revoked);
}
