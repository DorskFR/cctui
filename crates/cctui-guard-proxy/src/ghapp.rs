//! GitHub App installation-token provider (CCT-722). Instead of injecting a
//! long-lived PAT for the `github` service, the sidecar mints a short-lived,
//! repo-scoped GitHub App *installation token* at use-time and injects THAT, so
//! even in-session misuse (which the boundary can't prevent) is time- and
//! scope-bounded.
//!
//! Mechanism: the App private key (PEM) lives in the secret store — fetched by
//! the sidecar via its configured key [`SecretRef`], never on the worker. At
//! use-time the sidecar signs a short RS256 JWT (`iss`=App id,
//! ~9 min lifetime), exchanges it at `POST /app/installations/<id>/access_tokens`
//! (optionally scoping to `repositories`), and injects the returned ~1h
//! installation token. The token is cached until ~5 min before its `expires_at`
//! and re-minted on expiry — never per request. Neither the token nor the
//! private key is ever written to disk.
//!
//! Fail-closed and inert-by-default: if the App key is absent the key fetch is
//! `NotFound`, which the injector treats as "fall through to the normal
//! `github` `SecretBackend` fetch" (today's PAT/passthrough behavior). If the
//! token exchange itself fails (e.g. 401) that is a `Backend` error and the
//! injector forwards the agent's original header unchanged — never a blank.
//!
//! Personal-repo caveat: a GitHub App bot cannot CREATE PRs on a repo it isn't a
//! collaborator on. Where that bites, the injected `NanachiBot` machine-user PAT
//! path stays the fallback — keep the App path opt-in per identity.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio::time::Instant;

use crate::secrets::{Credential, SecretError, SecretRef, SecretSource};

/// Default GitHub REST base; overridable so tests can point at a mock server.
pub const DEFAULT_API_BASE: &str = "https://api.github.com";
/// Injection service key this provider mints for.
const DEFAULT_SERVICE: &str = "github";
/// Re-mint this long before the token's `expires_at`.
const EXPIRY_SKEW: Duration = Duration::from_secs(300);
/// JWT lifetime — GitHub caps App JWTs at 10 min; stay under it.
const JWT_TTL_SECS: i64 = 540;
/// Backdate `iat` to tolerate minor clock skew against GitHub.
const JWT_BACKDATE_SECS: i64 = 60;

/// Static configuration for the GitHub-App minting path. All fields come from
/// CLI/env except the private key, which is pulled from the secret store at
/// use-time. Absent config ⇒ the provider is never constructed (inert).
#[derive(Debug, Clone)]
pub struct GhAppConfig {
    /// GitHub App id (the JWT `iss`).
    pub app_id: String,
    /// Installation id the token is minted for.
    pub installation_id: String,
    /// Optional repository names to scope the token to (empty = installation
    /// default). Bare `name`, not `owner/name`, per the GitHub API.
    pub repositories: Vec<String>,
    /// REST base URL; [`DEFAULT_API_BASE`] in production.
    pub api_base: String,
    /// Injection service key this provider handles (default `github`).
    pub service: String,
}

impl GhAppConfig {
    /// Builds a config with production defaults for `api_base`/`service`.
    #[must_use]
    pub fn new(app_id: String, installation_id: String, repositories: Vec<String>) -> Self {
        Self {
            app_id,
            installation_id,
            repositories,
            api_base: DEFAULT_API_BASE.to_owned(),
            service: DEFAULT_SERVICE.to_owned(),
        }
    }
}

#[derive(Serialize)]
struct JwtClaims {
    iat: i64,
    exp: i64,
    iss: String,
}

#[derive(Serialize)]
struct TokenRequest<'a> {
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    repositories: &'a [String],
}

#[derive(Deserialize)]
struct TokenResponse {
    token: String,
    /// RFC3339 timestamp, e.g. `2026-07-17T12:00:00Z`.
    expires_at: String,
}

#[derive(Hash, PartialEq, Eq, Clone)]
struct CacheKey {
    installation: String,
    scope: String,
}

struct CacheEntry {
    valid_until: Instant,
    credential: Credential,
}

/// Mints and caches GitHub App installation tokens. Cache is expiry-aware and
/// in-memory only.
pub struct GhAppMinter {
    http: reqwest::Client,
    secrets: Arc<SecretSource>,
    config: GhAppConfig,
    key_ref: SecretRef,
    cache: Mutex<HashMap<CacheKey, CacheEntry>>,
}

impl std::fmt::Debug for GhAppMinter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GhAppMinter").field("config", &self.config).finish_non_exhaustive()
    }
}

impl GhAppMinter {
    pub fn new(
        secrets: Arc<SecretSource>,
        config: GhAppConfig,
        key_ref: SecretRef,
    ) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .pool_max_idle_per_host(0)
            .user_agent("cctui-guard-proxy")
            .build()?;
        Ok(Self { http, secrets, config, key_ref, cache: Mutex::new(HashMap::new()) })
    }

    /// True if this provider mints for `service` (the injection service key).
    #[must_use]
    pub fn handles(&self, service: &str) -> bool {
        service.eq_ignore_ascii_case(&self.config.service)
    }

    fn backend_err(context: &str, e: impl std::fmt::Display) -> SecretError {
        SecretError::Backend(anyhow::anyhow!("github app: {context}: {e}"))
    }

    fn scope_key(&self) -> String {
        self.config.repositories.join(",")
    }

    /// Returns a valid installation token, minting one if the cache is empty
    /// or within [`EXPIRY_SKEW`] of expiry.
    ///
    /// Errors mirror [`RefResolver`](crate::secrets::RefResolver): the App
    /// private key being absent is `NotFound` (the injector falls back to the
    /// normal `github` fetch), and any exchange/signing failure is `Backend`
    /// (the injector forwards the agent's original header — fail closed).
    pub async fn mint(&self) -> Result<Credential, SecretError> {
        let key =
            CacheKey { installation: self.config.installation_id.clone(), scope: self.scope_key() };
        {
            let cache = self.cache.lock().await;
            if let Some(entry) = cache.get(&key)
                && Instant::now() < entry.valid_until
            {
                return Ok(entry.credential.clone());
            }
        }

        // NotFound here (no App key configured) propagates so the injector falls
        // back to the stored `github` credential — this is what keeps the whole
        // path inert until an operator provisions the key.
        let pem = self.secrets.fetch(&self.key_ref).await?;
        let jwt = self.build_jwt(pem.expose())?;
        let (token, cache_for) = self.exchange(&jwt).await?;

        let credential = Credential::new(token);
        let entry =
            CacheEntry { valid_until: Instant::now() + cache_for, credential: credential.clone() };
        self.cache.lock().await.insert(key, entry);
        Ok(credential)
    }

    fn build_jwt(&self, pem: &str) -> Result<String, SecretError> {
        use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};

        let key = EncodingKey::from_rsa_pem(pem.as_bytes())
            .map_err(|e| Self::backend_err("invalid App private key", e))?;
        let now = chrono::Utc::now().timestamp();
        let claims = JwtClaims {
            iat: now - JWT_BACKDATE_SECS,
            exp: now + JWT_TTL_SECS,
            iss: self.config.app_id.clone(),
        };
        encode(&Header::new(Algorithm::RS256), &claims, &key)
            .map_err(|e| Self::backend_err("signing App JWT", e))
    }

    /// Exchanges the App JWT for an installation token. Returns the token and
    /// how long it is safe to cache it (its lifetime minus [`EXPIRY_SKEW`]).
    async fn exchange(&self, jwt: &str) -> Result<(String, Duration), SecretError> {
        let url = format!(
            "{}/app/installations/{}/access_tokens",
            self.config.api_base.trim_end_matches('/'),
            self.config.installation_id
        );
        let resp = self
            .http
            .post(&url)
            .bearer_auth(jwt)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .json(&TokenRequest { repositories: &self.config.repositories })
            .send()
            .await
            .map_err(|e| Self::backend_err("token exchange request", e))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(SecretError::Backend(anyhow::anyhow!(
                "github app: token exchange failed: HTTP {status}"
            )));
        }
        let body: TokenResponse =
            resp.json().await.map_err(|e| Self::backend_err("token exchange response", e))?;
        Ok((body.token, cache_duration(&body.expires_at)))
    }
}

/// How long to cache a token given its `expires_at`: the remaining lifetime
/// minus [`EXPIRY_SKEW`], clamped at zero (an already-near-expiry token is not
/// cached, forcing a re-mint next call).
fn cache_duration(expires_at: &str) -> Duration {
    let Ok(expires) = chrono::DateTime::parse_from_rfc3339(expires_at) else {
        // Unparseable expiry: don't cache — re-mint every call rather than risk
        // serving a stale token.
        return Duration::ZERO;
    };
    let remaining = expires.signed_duration_since(chrono::Utc::now());
    remaining.to_std().unwrap_or(Duration::ZERO).saturating_sub(EXPIRY_SKEW)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::{RefResolver, SecretError};

    fn key_ref() -> SecretRef {
        SecretRef::Env { var: "APP_KEY".to_owned() }
    }

    /// A resolver that returns a fixed value for the App-key ref and `NotFound`
    /// for everything else.
    struct KeyBackend {
        pem: Option<String>,
    }

    #[async_trait::async_trait]
    impl RefResolver for KeyBackend {
        async fn resolve(&self, r: &SecretRef) -> Result<Credential, SecretError> {
            match (&self.pem, r == &key_ref()) {
                (Some(pem), true) => Ok(Credential::new(pem.clone())),
                _ => Err(SecretError::NotFound { what: r.to_string() }),
            }
        }
    }

    fn test_rsa_keypair() -> (String, String) {
        use rsa::RsaPrivateKey;
        use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};

        let mut rng = rand::thread_rng();
        let key = RsaPrivateKey::new(&mut rng, 2048).unwrap();
        let private_pem = key.to_pkcs8_pem(LineEnding::LF).unwrap().to_string();
        let public_pem = key.to_public_key().to_public_key_pem(LineEnding::LF).unwrap();
        (private_pem, public_pem)
    }

    fn minter(secrets: Arc<SecretSource>, api_base: String) -> GhAppMinter {
        let mut config = GhAppConfig::new("123456".to_owned(), "42".to_owned(), Vec::new());
        config.api_base = api_base;
        GhAppMinter::new(secrets, config, key_ref()).unwrap()
    }

    fn source(backend: KeyBackend) -> Arc<SecretSource> {
        Arc::new(SecretSource::new(Box::new(backend), Duration::from_secs(120)))
    }

    #[test]
    fn jwt_is_well_formed() {
        use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};

        #[derive(serde::Deserialize)]
        struct Claims {
            iat: i64,
            exp: i64,
            iss: String,
        }

        let (private_pem, public_pem) = test_rsa_keypair();
        let m = minter(source(KeyBackend { pem: None }), DEFAULT_API_BASE.to_owned());
        let jwt = m.build_jwt(&private_pem).unwrap();

        let header = decode_header(&jwt).unwrap();
        assert_eq!(header.alg, Algorithm::RS256);

        let mut val = Validation::new(Algorithm::RS256);
        val.validate_aud = false;
        let data = decode::<Claims>(
            &jwt,
            &DecodingKey::from_rsa_pem(public_pem.as_bytes()).unwrap(),
            &val,
        )
        .unwrap();
        assert_eq!(data.claims.iss, "123456", "iss must be the App id");
        assert!(data.claims.exp > data.claims.iat, "exp must be after iat");
    }

    async fn spawn_github_mock(
        status: u16,
        expiry_secs: i64,
    ) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
        use std::sync::atomic::{AtomicUsize, Ordering};

        use axum::extract::State;
        use axum::http::{HeaderMap, StatusCode};
        use axum::response::IntoResponse;
        use axum::routing::post;
        use axum::{Json, Router};

        let calls = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route(
                "/app/installations/{id}/access_tokens",
                post(
                    move |State((calls, status, expiry_secs)): State<(
                        Arc<AtomicUsize>,
                        u16,
                        i64,
                    )>,
                          headers: HeaderMap| async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        if headers
                            .get("authorization")
                            .and_then(|v| v.to_str().ok())
                            .is_none_or(|v| !v.starts_with("Bearer "))
                        {
                            return (StatusCode::UNAUTHORIZED, "no bearer").into_response();
                        }
                        if status != 201 {
                            return (StatusCode::from_u16(status).unwrap(), "forced failure")
                                .into_response();
                        }
                        let expires = (chrono::Utc::now() + chrono::Duration::seconds(expiry_secs))
                            .to_rfc3339();
                        (
                            StatusCode::CREATED,
                            Json(serde_json::json!({
                                "token": "ghs_installation_token",
                                "expires_at": expires,
                            })),
                        )
                            .into_response()
                    },
                ),
            )
            .with_state((calls.clone(), status, expiry_secs));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}"), calls)
    }

    #[tokio::test]
    async fn mint_exchanges_jwt_for_installation_token() {
        let (private_pem, _) = test_rsa_keypair();
        let (base, _calls) = spawn_github_mock(201, 3600).await;
        let m = minter(source(KeyBackend { pem: Some(private_pem) }), base);
        let cred = m.mint().await.unwrap();
        assert_eq!(cred.expose(), "ghs_installation_token");
    }

    #[tokio::test]
    async fn missing_app_key_is_not_found_so_injector_falls_back() {
        let (base, calls) = spawn_github_mock(201, 3600).await;
        let m = minter(source(KeyBackend { pem: None }), base);
        assert!(matches!(m.mint().await, Err(SecretError::NotFound { .. })));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn exchange_401_fails_closed() {
        let (private_pem, _) = test_rsa_keypair();
        let (base, _calls) = spawn_github_mock(401, 3600).await;
        let m = minter(source(KeyBackend { pem: Some(private_pem) }), base);
        assert!(matches!(m.mint().await, Err(SecretError::Backend(_))));
    }

    #[tokio::test]
    async fn cache_reuses_within_lifetime() {
        use std::sync::atomic::Ordering;

        let (private_pem, _) = test_rsa_keypair();
        let (base, calls) = spawn_github_mock(201, 3600).await;
        let m = minter(source(KeyBackend { pem: Some(private_pem) }), base);

        assert_eq!(m.mint().await.unwrap().expose(), "ghs_installation_token");
        assert_eq!(m.mint().await.unwrap().expose(), "ghs_installation_token");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "second call within lifetime is cached");
    }

    #[tokio::test]
    async fn remints_after_expiry_horizon() {
        use std::sync::atomic::Ordering;

        // expires just past the skew ⇒ cache_for ≈ 1s; crossing it forces a
        // fresh exchange without a long real-time sleep.
        let (private_pem, _) = test_rsa_keypair();
        let (base, calls) = spawn_github_mock(201, 301).await;
        let m = minter(source(KeyBackend { pem: Some(private_pem) }), base);

        assert_eq!(m.mint().await.unwrap().expose(), "ghs_installation_token");
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert_eq!(m.mint().await.unwrap().expose(), "ghs_installation_token");
        assert_eq!(calls.load(Ordering::SeqCst), 2, "past the expiry horizon must re-mint");
    }

    #[test]
    fn cache_duration_subtracts_skew_and_clamps() {
        let soon = (chrono::Utc::now() + chrono::Duration::seconds(3600)).to_rfc3339();
        let d = cache_duration(&soon);
        assert!(d <= Duration::from_secs(3600 - 300));
        assert!(d >= Duration::from_secs(3600 - 300 - 30), "≈ lifetime minus skew");

        let past = (chrono::Utc::now() - chrono::Duration::seconds(10)).to_rfc3339();
        assert_eq!(cache_duration(&past), Duration::ZERO, "expired ⇒ no caching");
        assert_eq!(cache_duration("not-a-date"), Duration::ZERO, "unparseable ⇒ no caching");
    }

    #[test]
    fn handles_matches_github_service_case_insensitively() {
        let m = minter(source(KeyBackend { pem: None }), DEFAULT_API_BASE.to_owned());
        assert!(m.handles("github"));
        assert!(m.handles("GitHub"));
        assert!(!m.handles("npm"));
    }
}
