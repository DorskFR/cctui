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
            host: Host { pool, cleared: DashMap::new() },
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
