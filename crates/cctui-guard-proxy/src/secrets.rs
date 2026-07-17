//! Pluggable secret source (CCT-717; consumed by credential injection in
//! CCT-718). Invariants the types cannot express: secrets stay in memory only
//! (TTL cache, never persisted), `Credential` redacts `Debug` so values never
//! reach logs, and a backend failure is `SecretError::Backend` — distinct from
//! `NotFound` — so callers fail closed instead of injecting a blank secret. A
//! fetch past TTL re-reads the backend, so store-side rotation lands within
//! one TTL.

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use tokio::sync::Mutex;
use tokio::time::Instant;

/// A secret value. `Debug` is redacted; use [`Credential::expose`] at the
/// injection point only.
#[derive(Clone)]
pub struct Credential {
    value: String,
}

impl Credential {
    pub const fn new(value: String) -> Self {
        Self { value }
    }

    pub fn expose(&self) -> &str {
        &self.value
    }
}

impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Credential(<redacted>)")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    /// No credential is configured for this (identity, service). The caller
    /// may treat the service as credential-less.
    #[error("no credential configured for {identity}/{service}")]
    NotFound { identity: String, service: String },
    /// The backend failed. The caller must fail closed — never inject a blank
    /// or stale-beyond-TTL secret.
    #[error("secret backend failure: {0}")]
    Backend(anyhow::Error),
}

impl SecretError {
    fn not_found(identity: &str, service: &str) -> Self {
        Self::NotFound { identity: identity.to_owned(), service: service.to_owned() }
    }
}

#[async_trait::async_trait]
pub trait SecretBackend: Send + Sync {
    async fn fetch(&self, identity: &str, service: &str) -> Result<Credential, SecretError>;
}

/// TTL-cached front for a [`SecretBackend`]. In-memory only.
pub struct SecretSource {
    backend: Box<dyn SecretBackend>,
    ttl: Duration,
    cache: Mutex<HashMap<(String, String), CacheEntry>>,
}

struct CacheEntry {
    fetched_at: Instant,
    credential: Credential,
}

impl SecretSource {
    pub fn new(backend: Box<dyn SecretBackend>, ttl: Duration) -> Self {
        Self { backend, ttl, cache: Mutex::new(HashMap::new()) }
    }

    pub async fn fetch(&self, identity: &str, service: &str) -> Result<Credential, SecretError> {
        let key = (identity.to_owned(), service.to_owned());
        let mut cache = self.cache.lock().await;
        if let Some(entry) = cache.get(&key)
            && entry.fetched_at.elapsed() < self.ttl
        {
            return Ok(entry.credential.clone());
        }
        match self.backend.fetch(identity, service).await {
            Ok(credential) => {
                cache.insert(
                    key,
                    CacheEntry { fetched_at: Instant::now(), credential: credential.clone() },
                );
                Ok(credential)
            }
            Err(e) => {
                // Fail closed: drop any expired entry rather than serving it.
                cache.remove(&key);
                drop(cache);
                Err(e)
            }
        }
    }
}

type EnvLookup = Box<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Resolves `CRED_<IDENTITY>_<SERVICE>` from the sidecar's own environment.
pub struct EnvBackend {
    lookup: EnvLookup,
}

impl EnvBackend {
    pub fn from_process_env() -> Self {
        Self { lookup: Box::new(|name| std::env::var(name).ok()) }
    }

    #[cfg(test)]
    fn with_lookup(lookup: EnvLookup) -> Self {
        Self { lookup }
    }

    /// `(identity, service)` → `CRED_<IDENTITY>_<SERVICE>`: ASCII
    /// alphanumerics uppercased, everything else `_`.
    pub fn var_name(identity: &str, service: &str) -> String {
        fn sanitize(part: &str) -> String {
            part.chars()
                .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' })
                .collect()
        }
        format!("CRED_{}_{}", sanitize(identity), sanitize(service))
    }
}

#[async_trait::async_trait]
impl SecretBackend for EnvBackend {
    async fn fetch(&self, identity: &str, service: &str) -> Result<Credential, SecretError> {
        let name = Self::var_name(identity, service);
        match (self.lookup)(&name) {
            Some(value) if !value.is_empty() => Ok(Credential::new(value)),
            _ => Err(SecretError::not_found(identity, service)),
        }
    }
}

#[derive(Debug, Clone)]
pub struct VaultConfig {
    /// Base address, e.g. `https://vault.vault:8200`.
    pub addr: String,
    /// Kubernetes auth role.
    pub role: String,
    /// KV v2 mount, e.g. `kvmount`.
    pub mount: String,
    /// Path prefix under the mount; final path is
    /// `<mount>/data/<prefix>/<identity>/<service>`.
    pub path_prefix: String,
    /// Field to read from the KV v2 secret data.
    pub field: String,
    /// Pod service account token used for the Kubernetes login.
    pub token_path: PathBuf,
}

/// `HashiCorp` Vault / `OpenBao` KV v2 over plain HTTP (Kubernetes auth).
pub struct VaultBackend {
    http: reqwest::Client,
    config: VaultConfig,
}

impl VaultBackend {
    pub fn new(config: VaultConfig) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder().timeout(Duration::from_secs(10)).build()?;
        Ok(Self { http, config })
    }

    fn backend_err(context: &str, e: impl fmt::Display) -> SecretError {
        SecretError::Backend(anyhow::anyhow!("{context}: {e}"))
    }

    async fn login(&self) -> Result<String, SecretError> {
        let jwt = tokio::fs::read_to_string(&self.config.token_path)
            .await
            .map_err(|e| Self::backend_err("reading service account token", e))?;
        let url = format!("{}/v1/auth/kubernetes/login", self.config.addr.trim_end_matches('/'));
        let resp = self
            .http
            .post(&url)
            .json(&serde_json::json!({ "jwt": jwt.trim(), "role": self.config.role }))
            .send()
            .await
            .map_err(|e| Self::backend_err("vault login request", e))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(SecretError::Backend(anyhow::anyhow!("vault login failed: HTTP {status}")));
        }
        let body: serde_json::Value =
            resp.json().await.map_err(|e| Self::backend_err("vault login response", e))?;
        body.pointer("/auth/client_token")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| {
                SecretError::Backend(anyhow::anyhow!(
                    "vault login response missing auth.client_token"
                ))
            })
    }

    fn secret_url(&self, identity: &str, service: &str) -> String {
        let prefix = self.config.path_prefix.trim_matches('/');
        let path = if prefix.is_empty() {
            format!("{identity}/{service}")
        } else {
            format!("{prefix}/{identity}/{service}")
        };
        format!(
            "{}/v1/{}/data/{path}",
            self.config.addr.trim_end_matches('/'),
            self.config.mount.trim_matches('/')
        )
    }
}

#[async_trait::async_trait]
impl SecretBackend for VaultBackend {
    async fn fetch(&self, identity: &str, service: &str) -> Result<Credential, SecretError> {
        let token = self.login().await?;
        let resp = self
            .http
            .get(self.secret_url(identity, service))
            .header("X-Vault-Token", &token)
            .send()
            .await
            .map_err(|e| Self::backend_err("vault read request", e))?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(SecretError::not_found(identity, service));
        }
        if !status.is_success() {
            return Err(SecretError::Backend(anyhow::anyhow!("vault read failed: HTTP {status}")));
        }
        let body: serde_json::Value =
            resp.json().await.map_err(|e| Self::backend_err("vault read response", e))?;
        match body
            .pointer(&format!("/data/data/{}", self.config.field))
            .and_then(serde_json::Value::as_str)
        {
            Some(value) if !value.is_empty() => Ok(Credential::new(value.to_owned())),
            _ => Err(SecretError::not_found(identity, service)),
        }
    }
}

/// AWS Secrets Manager via the sidecar's ambient identity (SDK default chain:
/// EKS Pod Identity / IRSA / env). Secret name: `<prefix><identity>/<service>`;
/// per-identity authorization lives in IAM, not here.
pub struct AwsSmBackend {
    client: aws_sdk_secretsmanager::Client,
    prefix: String,
}

impl AwsSmBackend {
    pub async fn from_default_chain(prefix: String) -> Self {
        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        Self { client: aws_sdk_secretsmanager::Client::new(&config), prefix: normalize_prefix(prefix) }
    }

    pub fn secret_name(prefix: &str, identity: &str, service: &str) -> String {
        format!("{prefix}{identity}/{service}")
    }
}

fn normalize_prefix(prefix: String) -> String {
    if prefix.is_empty() || prefix.ends_with('/') { prefix } else { format!("{prefix}/") }
}

#[async_trait::async_trait]
impl SecretBackend for AwsSmBackend {
    async fn fetch(&self, identity: &str, service: &str) -> Result<Credential, SecretError> {
        let name = Self::secret_name(&self.prefix, identity, service);
        match self.client.get_secret_value().secret_id(&name).send().await {
            Ok(out) => match out.secret_string() {
                Some(value) if !value.is_empty() => Ok(Credential::new(value.to_owned())),
                _ => Err(SecretError::not_found(identity, service)),
            },
            Err(e) if e.as_service_error().is_some_and(
                aws_sdk_secretsmanager::operation::get_secret_value::GetSecretValueError::is_resource_not_found_exception,
            ) => Err(SecretError::not_found(identity, service)),
            Err(e) => Err(SecretError::Backend(anyhow::anyhow!(
                "aws secrets manager get {name}: {}",
                aws_sdk_secretsmanager::error::DisplayErrorContext(&e)
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    struct MockBackend {
        response: StdMutex<Result<String, &'static str>>,
        calls: AtomicUsize,
    }

    impl MockBackend {
        fn ok(value: &str) -> Self {
            Self { response: StdMutex::new(Ok(value.to_owned())), calls: AtomicUsize::new(0) }
        }

        fn failing(msg: &'static str) -> Self {
            Self { response: StdMutex::new(Err(msg)), calls: AtomicUsize::new(0) }
        }
    }

    #[async_trait::async_trait]
    impl SecretBackend for MockBackend {
        async fn fetch(&self, _identity: &str, _service: &str) -> Result<Credential, SecretError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            match &*self.response.lock().unwrap() {
                Ok(v) => Ok(Credential::new(v.clone())),
                Err(msg) => Err(SecretError::Backend(anyhow::anyhow!(*msg))),
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn cache_serves_fresh_hit_without_backend_call() {
        let backend = std::sync::Arc::new(MockBackend::ok("s3cret"));
        let source = SecretSource::new(Box::new(ArcBackend(backend.clone())), Duration::from_secs(120));
        assert_eq!(source.fetch("acme", "github").await.unwrap().expose(), "s3cret");
        tokio::time::advance(Duration::from_secs(119)).await;
        assert_eq!(source.fetch("acme", "github").await.unwrap().expose(), "s3cret");
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn cache_expiry_refetches_and_picks_up_rotation() {
        let backend = std::sync::Arc::new(MockBackend::ok("v1"));
        let source = SecretSource::new(Box::new(ArcBackend(backend.clone())), Duration::from_secs(120));
        assert_eq!(source.fetch("acme", "github").await.unwrap().expose(), "v1");

        *backend.response.lock().unwrap() = Ok("v2".to_owned());
        tokio::time::advance(Duration::from_secs(119)).await;
        assert_eq!(
            source.fetch("acme", "github").await.unwrap().expose(),
            "v1",
            "rotation must NOT land before TTL expiry"
        );
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);

        tokio::time::advance(Duration::from_secs(2)).await;
        assert_eq!(
            source.fetch("acme", "github").await.unwrap().expose(),
            "v2",
            "rotation must land after TTL expiry"
        );
        assert_eq!(backend.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn backend_error_fails_closed_and_is_not_cached() {
        let backend = std::sync::Arc::new(MockBackend::failing("boom"));
        let source = SecretSource::new(Box::new(ArcBackend(backend.clone())), Duration::from_secs(120));
        assert!(matches!(
            source.fetch("acme", "github").await,
            Err(SecretError::Backend(_))
        ));
        *backend.response.lock().unwrap() = Ok("recovered".to_owned());
        assert_eq!(source.fetch("acme", "github").await.unwrap().expose(), "recovered");
        assert_eq!(backend.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn backend_error_past_ttl_drops_stale_entry() {
        let backend = std::sync::Arc::new(MockBackend::ok("v1"));
        let source = SecretSource::new(Box::new(ArcBackend(backend.clone())), Duration::from_secs(120));
        assert_eq!(source.fetch("acme", "github").await.unwrap().expose(), "v1");
        *backend.response.lock().unwrap() = Err("backend down");
        tokio::time::advance(Duration::from_secs(121)).await;
        assert!(matches!(
            source.fetch("acme", "github").await,
            Err(SecretError::Backend(_))
        ));
    }

    struct ArcBackend(std::sync::Arc<MockBackend>);

    #[async_trait::async_trait]
    impl SecretBackend for ArcBackend {
        async fn fetch(&self, identity: &str, service: &str) -> Result<Credential, SecretError> {
            self.0.fetch(identity, service).await
        }
    }

    #[test]
    fn credential_debug_is_redacted() {
        let c = Credential::new("hunter2".to_owned());
        assert_eq!(format!("{c:?}"), "Credential(<redacted>)");
    }

    #[test]
    fn env_var_name_convention() {
        assert_eq!(EnvBackend::var_name("acme-corp", "github"), "CRED_ACME_CORP_GITHUB");
        assert_eq!(EnvBackend::var_name("Foo.Bar", "npm registry"), "CRED_FOO_BAR_NPM_REGISTRY");
    }

    #[tokio::test]
    async fn env_backend_resolves_and_fails_closed() {
        let backend = EnvBackend::with_lookup(Box::new(|name| match name {
            "CRED_ACME_GITHUB" => Some("tok".to_owned()),
            "CRED_ACME_EMPTY" => Some(String::new()),
            _ => None,
        }));
        assert_eq!(backend.fetch("acme", "github").await.unwrap().expose(), "tok");
        assert!(matches!(
            backend.fetch("acme", "empty").await,
            Err(SecretError::NotFound { .. })
        ));
        assert!(matches!(
            backend.fetch("acme", "missing").await,
            Err(SecretError::NotFound { .. })
        ));
    }

    #[test]
    fn aws_secret_name_mapping() {
        assert_eq!(AwsSmBackend::secret_name("cctui/worker/", "acme", "github"), "cctui/worker/acme/github");
        assert_eq!(normalize_prefix("cctui/worker".to_owned()), "cctui/worker/");
        assert_eq!(normalize_prefix("cctui/worker/".to_owned()), "cctui/worker/");
        assert_eq!(normalize_prefix(String::new()), "");
    }

    async fn spawn_vault_mock() -> String {
        use axum::extract::Path;
        use axum::http::{HeaderMap, StatusCode};
        use axum::routing::{get, post};
        use axum::{Json, Router};

        let app = Router::new()
            .route(
                "/v1/auth/kubernetes/login",
                post(|Json(body): Json<serde_json::Value>| async move {
                    if body["jwt"] == "test-jwt" && body["role"] == "cctui-worker" {
                        Json(serde_json::json!({"auth": {"client_token": "tok-123"}}))
                            .into_response()
                    } else {
                        (StatusCode::FORBIDDEN, "bad login").into_response()
                    }
                }),
            )
            .route(
                "/v1/kvmount/data/cctui/workers/{identity}/{service}",
                get(
                    |Path((identity, service)): Path<(String, String)>, headers: HeaderMap| async move {
                        if headers.get("X-Vault-Token").and_then(|v| v.to_str().ok())
                            != Some("tok-123")
                        {
                            return (StatusCode::FORBIDDEN, "bad token").into_response();
                        }
                        match (identity.as_str(), service.as_str()) {
                            ("acme", "github") => Json(
                                serde_json::json!({"data": {"data": {"value": "hunter2"}}}),
                            )
                            .into_response(),
                            ("boom", _) => {
                                (StatusCode::INTERNAL_SERVER_ERROR, "vault sad").into_response()
                            }
                            _ => (StatusCode::NOT_FOUND, "no secret").into_response(),
                        }
                    },
                ),
            );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    }

    use axum::response::IntoResponse;

    fn vault_backend(addr: String, token_path: PathBuf) -> VaultBackend {
        VaultBackend::new(VaultConfig {
            addr,
            role: "cctui-worker".to_owned(),
            mount: "kvmount".to_owned(),
            path_prefix: "cctui/workers".to_owned(),
            field: "value".to_owned(),
            token_path,
        })
        .unwrap()
    }

    fn write_token(dir: &tempfile::TempDir, contents: &str) -> PathBuf {
        let path = dir.path().join("token");
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[tokio::test]
    async fn vault_backend_logs_in_and_reads_kv2() {
        let addr = spawn_vault_mock().await;
        let dir = tempfile::tempdir().unwrap();
        let backend = vault_backend(addr, write_token(&dir, "test-jwt\n"));
        assert_eq!(backend.fetch("acme", "github").await.unwrap().expose(), "hunter2");
    }

    #[tokio::test]
    async fn vault_backend_missing_secret_is_not_found() {
        let addr = spawn_vault_mock().await;
        let dir = tempfile::tempdir().unwrap();
        let backend = vault_backend(addr, write_token(&dir, "test-jwt"));
        assert!(matches!(
            backend.fetch("acme", "unknown").await,
            Err(SecretError::NotFound { .. })
        ));
    }

    #[tokio::test]
    async fn vault_backend_server_error_fails_closed() {
        let addr = spawn_vault_mock().await;
        let dir = tempfile::tempdir().unwrap();
        let backend = vault_backend(addr, write_token(&dir, "test-jwt"));
        assert!(matches!(
            backend.fetch("boom", "github").await,
            Err(SecretError::Backend(_))
        ));
    }

    #[tokio::test]
    async fn vault_backend_login_rejection_fails_closed() {
        let addr = spawn_vault_mock().await;
        let dir = tempfile::tempdir().unwrap();
        let backend = vault_backend(addr, write_token(&dir, "wrong-jwt"));
        assert!(matches!(
            backend.fetch("acme", "github").await,
            Err(SecretError::Backend(_))
        ));
    }

    #[tokio::test]
    async fn vault_backend_unreadable_token_fails_closed() {
        let addr = spawn_vault_mock().await;
        let backend = vault_backend(addr, PathBuf::from("/nonexistent/token"));
        assert!(matches!(
            backend.fetch("acme", "github").await,
            Err(SecretError::Backend(_))
        ));
    }

    #[test]
    fn vault_secret_url_shape() {
        let dir = tempfile::tempdir().unwrap();
        let backend = vault_backend("http://vault:8200/".to_owned(), write_token(&dir, "x"));
        assert_eq!(
            backend.secret_url("acme", "github"),
            "http://vault:8200/v1/kvmount/data/cctui/workers/acme/github"
        );
    }
}
