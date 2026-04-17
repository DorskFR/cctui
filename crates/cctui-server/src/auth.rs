use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

/// Cache TTL for positive auth lookups. Bounds revocation latency.
const CACHE_TTL: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenRole {
    Admin,
    /// Legacy env-based agent token (`CCTUI_AGENT_TOKENS`). Kept for backward
    /// compatibility while channel/TUI are migrated to machine keys (PR2).
    Agent,
    User,
    Machine,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct AuthContext {
    pub role: TokenRole,
    pub user_id: Option<Uuid>,
    pub machine_id: Option<Uuid>,
}

#[derive(Clone)]
struct CacheEntry {
    ctx: AuthContext,
    expires: Instant,
}

#[derive(Clone)]
pub struct AuthConfig {
    pub admin_tokens: Vec<String>,
    pub agent_tokens: Vec<String>,
    pub pool: PgPool,
    cache: Arc<Mutex<HashMap<String, CacheEntry>>>,
}

impl AuthConfig {
    pub fn new(admin_tokens: Vec<String>, agent_tokens: Vec<String>, pool: PgPool) -> Self {
        Self { admin_tokens, agent_tokens, pool, cache: Arc::new(Mutex::new(HashMap::new())) }
    }

    /// Purge a cached entry by its key hash. Call after revoke/rotate so the
    /// change takes effect immediately rather than after TTL.
    pub fn purge(&self, key_hash: &str) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.remove(key_hash);
        }
    }

    fn cache_get(&self, hash: &str) -> Option<AuthContext> {
        let mut cache = self.cache.lock().ok()?;
        if let Some(entry) = cache.get(hash).cloned() {
            if entry.expires > Instant::now() {
                return Some(entry.ctx);
            }
            cache.remove(hash);
        }
        None
    }

    fn cache_put(&self, hash: String, ctx: AuthContext) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(hash, CacheEntry { ctx, expires: Instant::now() + CACHE_TTL });
        }
    }

    pub async fn validate(&self, token: &str) -> Option<AuthContext> {
        // Env-based admin/agent take precedence (cheap, no DB hit).
        if self.admin_tokens.iter().any(|t| t == token) {
            return Some(AuthContext { role: TokenRole::Admin, user_id: None, machine_id: None });
        }
        if self.agent_tokens.iter().any(|t| t == token) {
            return Some(AuthContext { role: TokenRole::Agent, user_id: None, machine_id: None });
        }

        let hash = sha256_hex(token);
        if let Some(ctx) = self.cache_get(&hash) {
            return Some(ctx);
        }

        // Machine lookup (joined with users so a revoked user also revokes all machines).
        let row: Option<(Uuid, Uuid)> = sqlx::query_as(
            "SELECT m.id, m.user_id FROM machines m \
             JOIN users u ON u.id = m.user_id \
             WHERE m.key_hash = $1 AND m.revoked_at IS NULL AND u.revoked_at IS NULL",
        )
        .bind(&hash)
        .fetch_optional(&self.pool)
        .await
        .unwrap_or(None);

        if let Some((machine_id, user_id)) = row {
            let ctx = AuthContext {
                role: TokenRole::Machine,
                user_id: Some(user_id),
                machine_id: Some(machine_id),
            };
            self.cache_put(hash.clone(), ctx.clone());
            // Best-effort last_seen update.
            let pool = self.pool.clone();
            tokio::spawn(async move {
                let _ = sqlx::query("UPDATE machines SET last_seen_at = now() WHERE id = $1")
                    .bind(machine_id)
                    .execute(&pool)
                    .await;
            });
            return Some(ctx);
        }

        // User lookup.
        let row: Option<(Uuid,)> =
            sqlx::query_as("SELECT id FROM users WHERE key_hash = $1 AND revoked_at IS NULL")
                .bind(&hash)
                .fetch_optional(&self.pool)
                .await
                .unwrap_or(None);

        if let Some((user_id,)) = row {
            let ctx =
                AuthContext { role: TokenRole::User, user_id: Some(user_id), machine_id: None };
            self.cache_put(hash, ctx.clone());
            return Some(ctx);
        }

        None
    }
}

pub async fn auth_middleware(request: Request, next: Next) -> Result<Response, StatusCode> {
    let auth_config = request
        .extensions()
        .get::<AuthConfig>()
        .cloned()
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;

    let token = request
        .headers()
        .get(http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?
        .to_string();

    let ctx = auth_config.validate(&token).await.ok_or(StatusCode::UNAUTHORIZED)?;

    let mut request = request;
    request.extensions_mut().insert(ctx);
    Ok(next.run(request).await)
}

/// Require Admin role; 403 otherwise.
pub const fn require_admin(ctx: &AuthContext) -> Result<(), StatusCode> {
    match ctx.role {
        TokenRole::Admin => Ok(()),
        _ => Err(StatusCode::FORBIDDEN),
    }
}

/// Require User role; 403 otherwise. Admin is not accepted — only humans enrol machines.
pub const fn require_user(ctx: &AuthContext) -> Result<Uuid, StatusCode> {
    match (ctx.role, ctx.user_id) {
        (TokenRole::User, Some(uid)) => Ok(uid),
        _ => Err(StatusCode::FORBIDDEN),
    }
}

/// Generate a new secret. 64 hex chars = 256 bits of entropy.
#[must_use]
pub fn mint_secret() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

#[must_use]
pub fn user_token(secret: &str) -> String {
    format!("cctui_u_{secret}")
}

#[must_use]
pub fn machine_token(secret: &str) -> String {
    format!("cctui_m_{secret}")
}

#[must_use]
pub fn sha256_hex(input: &str) -> String {
    let mut h = Sha256::new();
    h.update(input.as_bytes());
    hex::encode(h.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with_env_only() -> AuthConfig {
        // Pool is required but never touched for env-token paths.
        // Build a cheaply-invalid lazy pool — tests that hit DB are integration-gated.
        let pool = PgPool::connect_lazy("postgres://invalid").unwrap();
        AuthConfig::new(vec!["admin-secret".into()], vec!["agent-secret".into()], pool)
    }

    #[tokio::test]
    async fn env_admin_resolves() {
        let cfg = config_with_env_only();
        let ctx = cfg.validate("admin-secret").await.unwrap();
        assert_eq!(ctx.role, TokenRole::Admin);
        assert!(ctx.user_id.is_none());
    }

    #[tokio::test]
    async fn env_agent_resolves() {
        let cfg = config_with_env_only();
        let ctx = cfg.validate("agent-secret").await.unwrap();
        assert_eq!(ctx.role, TokenRole::Agent);
    }

    #[test]
    fn sha256_is_stable() {
        assert_eq!(sha256_hex("hello").len(), 64);
        assert_eq!(sha256_hex("hello"), sha256_hex("hello"));
    }

    #[test]
    fn tokens_have_prefix() {
        let s = mint_secret();
        assert_eq!(s.len(), 64);
        assert!(user_token(&s).starts_with("cctui_u_"));
        assert!(machine_token(&s).starts_with("cctui_m_"));
    }

    #[tokio::test]
    async fn cache_roundtrip() {
        let cfg = config_with_env_only();
        let ctx =
            AuthContext { role: TokenRole::User, user_id: Some(Uuid::nil()), machine_id: None };
        cfg.cache_put("h".into(), ctx);
        let got = cfg.cache_get("h").unwrap();
        assert_eq!(got.role, TokenRole::User);
        cfg.purge("h");
        assert!(cfg.cache_get("h").is_none());
    }
}
