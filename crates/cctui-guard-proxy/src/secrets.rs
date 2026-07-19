//! Pluggable secret resolution (CCT-717; consumed by credential injection in
//! CCT-718). Every secret the proxy can inject is named by an explicit
//! [`SecretRef`] — the proxy knows engines (env / Vault / AWS SM / Kubernetes
//! secrets), never a site-specific taxonomy. Invariants the types cannot
//! express: secrets stay in memory only (TTL cache, never persisted),
//! `Credential` redacts `Debug` so values never reach logs, and an engine
//! failure is `SecretError::Backend` — distinct from `NotFound` — so callers
//! fail closed instead of injecting a blank secret. A fetch past TTL re-reads
//! the engine, so store-side rotation lands within one TTL.

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use base64::Engine as _;
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
    /// The referenced secret does not exist. The caller may treat the service
    /// as credential-less.
    #[error("secret not found: {what}")]
    NotFound { what: String },
    /// The engine failed. The caller must fail closed — never inject a blank
    /// or stale-beyond-TTL secret.
    #[error("secret backend failure: {0}")]
    Backend(anyhow::Error),
}

impl SecretError {
    fn not_found(r: &SecretRef) -> Self {
        Self::NotFound { what: r.to_string() }
    }
}

/// Uppercases ASCII alphanumerics, maps everything else to `_` — the shared
/// identity/service → env-var-fragment convention.
fn sanitize_upper(part: &str) -> String {
    part.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' })
        .collect()
}

/// Substitutes `${IDENTITY}` (sanitized-uppercase) and `${identity}` (verbatim)
/// in a secret-ref template. A template that uses a placeholder while no
/// identity is set is an error — never silently resolve to a wrong path.
#[allow(clippy::literal_string_with_formatting_args)]
pub fn render_identity(template: &str, identity: &str) -> anyhow::Result<String> {
    if !template.contains("${") {
        return Ok(template.to_owned());
    }
    anyhow::ensure!(
        !identity.is_empty(),
        "secret ref {template:?} uses an identity placeholder but no identity is set"
    );
    let out =
        template.replace("${IDENTITY}", &sanitize_upper(identity)).replace("${identity}", identity);
    anyhow::ensure!(!out.contains("${"), "unresolved placeholder in secret ref {template:?}");
    Ok(out)
}

/// An explicit, engine-qualified secret address. The proxy carries no knowledge
/// of what the secret is FOR — the injection config maps hosts to refs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretRef {
    /// `env:VAR` — the sidecar's own environment.
    Env { var: String },
    /// `vault:<mount>/data/<path>#<field>` — Vault/OpenBao KV v2, the same
    /// shape the bank-vaults webhook uses.
    Vault { mount: String, path: String, field: String },
    /// `aws-sm:<name>` or `aws-sm:<name>#<json-field>` — AWS Secrets Manager.
    AwsSm { name: String, field: Option<String> },
    /// `k8s:[<namespace>/]<secret>#<key>` — a Kubernetes Secret read via the
    /// pod's own `ServiceAccount`.
    K8s { namespace: Option<String>, name: String, key: String },
}

impl SecretRef {
    /// Parses the canonical string form (see variant docs). The identity
    /// templating of [`render_identity`] must be applied BEFORE parsing.
    pub fn parse(s: &str) -> anyhow::Result<Self> {
        let s = s.trim();
        if let Some(var) = s.strip_prefix("env:") {
            anyhow::ensure!(!var.is_empty(), "empty env ref");
            return Ok(Self::Env { var: var.to_owned() });
        }
        // `bao:` aliases `vault:` for refs carried in POD ENV VALUES: the
        // bank-vaults webhook mutates any env value with a literal `vault:`
        // prefix, which would hijack the ref before the proxy ever sees it.
        if let Some(rest) = s.strip_prefix("vault:").or_else(|| s.strip_prefix("bao:")) {
            let (path_part, field) = rest
                .split_once('#')
                .ok_or_else(|| anyhow::anyhow!("vault ref {s:?} needs #<field>"))?;
            let (mount, path) = path_part
                .split_once("/data/")
                .ok_or_else(|| anyhow::anyhow!("vault ref {s:?} must be <mount>/data/<path>"))?;
            anyhow::ensure!(
                !mount.is_empty() && !path.is_empty() && !field.is_empty(),
                "vault ref {s:?} has an empty mount/path/field"
            );
            return Ok(Self::Vault {
                mount: mount.trim_matches('/').to_owned(),
                path: path.trim_matches('/').to_owned(),
                field: field.to_owned(),
            });
        }
        if let Some(rest) = s.strip_prefix("aws-sm:") {
            let (name, field) =
                rest.split_once('#').map_or((rest, None), |(n, f)| (n, Some(f.to_owned())));
            anyhow::ensure!(!name.is_empty(), "empty aws-sm ref");
            anyhow::ensure!(field.as_deref() != Some(""), "aws-sm ref {s:?} has an empty field");
            return Ok(Self::AwsSm { name: name.to_owned(), field });
        }
        if let Some(rest) = s.strip_prefix("k8s:") {
            let (left, key) = rest
                .split_once('#')
                .ok_or_else(|| anyhow::anyhow!("k8s ref {s:?} needs #<key>"))?;
            let (namespace, name) =
                left.rsplit_once('/').map_or((None, left), |(ns, n)| (Some(ns.to_owned()), n));
            anyhow::ensure!(
                !name.is_empty() && !key.is_empty() && namespace.as_deref() != Some(""),
                "k8s ref {s:?} has an empty secret/key/namespace"
            );
            return Ok(Self::K8s { namespace, name: name.to_owned(), key: key.to_owned() });
        }
        anyhow::bail!("unknown secret ref scheme in {s:?} (expected env:/vault:/aws-sm:/k8s:)")
    }

    /// Legacy `(identity, service)` → `env:CRED_<IDENTITY>_<SERVICE>`.
    #[must_use]
    pub fn legacy_env(identity: &str, service: &str) -> Self {
        Self::Env { var: format!("CRED_{}_{}", sanitize_upper(identity), sanitize_upper(service)) }
    }

    /// Legacy `(identity, service)` → `vault:<mount>/data/<prefix>/<identity>/<service>#<field>`.
    #[must_use]
    pub fn legacy_vault(
        mount: &str,
        prefix: &str,
        field: &str,
        identity: &str,
        service: &str,
    ) -> Self {
        let prefix = prefix.trim_matches('/');
        let path = if prefix.is_empty() {
            format!("{identity}/{service}")
        } else {
            format!("{prefix}/{identity}/{service}")
        };
        Self::Vault { mount: mount.trim_matches('/').to_owned(), path, field: field.to_owned() }
    }

    /// Legacy `(identity, service)` → `aws-sm:<prefix><identity>/<service>`.
    #[must_use]
    pub fn legacy_aws_sm(prefix: &str, identity: &str, service: &str) -> Self {
        let prefix = if prefix.is_empty() || prefix.ends_with('/') {
            prefix.to_owned()
        } else {
            format!("{prefix}/")
        };
        Self::AwsSm { name: format!("{prefix}{identity}/{service}"), field: None }
    }
}

impl fmt::Display for SecretRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Env { var } => write!(f, "env:{var}"),
            Self::Vault { mount, path, field } => write!(f, "vault:{mount}/data/{path}#{field}"),
            Self::AwsSm { name, field: None } => write!(f, "aws-sm:{name}"),
            Self::AwsSm { name, field: Some(field) } => write!(f, "aws-sm:{name}#{field}"),
            Self::K8s { namespace: None, name, key } => write!(f, "k8s:{name}#{key}"),
            Self::K8s { namespace: Some(ns), name, key } => write!(f, "k8s:{ns}/{name}#{key}"),
        }
    }
}

/// Resolves a [`SecretRef`] to a [`Credential`]. Object-safe so the TTL cache
/// and tests can wrap/mock it.
#[async_trait::async_trait]
pub trait RefResolver: Send + Sync {
    async fn resolve(&self, r: &SecretRef) -> Result<Credential, SecretError>;
}

/// TTL-cached front for a [`RefResolver`], keyed by the ref's canonical string.
/// In-memory only.
pub struct SecretSource {
    resolver: Box<dyn RefResolver>,
    ttl: Duration,
    cache: Mutex<HashMap<String, CacheEntry>>,
}

struct CacheEntry {
    fetched_at: Instant,
    credential: Credential,
}

impl SecretSource {
    pub fn new(resolver: Box<dyn RefResolver>, ttl: Duration) -> Self {
        Self { resolver, ttl, cache: Mutex::new(HashMap::new()) }
    }

    pub async fn fetch(&self, r: &SecretRef) -> Result<Credential, SecretError> {
        let key = r.to_string();
        let mut cache = self.cache.lock().await;
        if let Some(entry) = cache.get(&key)
            && entry.fetched_at.elapsed() < self.ttl
        {
            return Ok(entry.credential.clone());
        }
        match self.resolver.resolve(r).await {
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

/// All engines behind one resolver. Engines are independent: a ref names its
/// engine, so one process can serve `env:` and `vault:` refs side by side. An
/// engine that is not configured fails its refs as `Backend` (closed).
pub struct Engines {
    env_lookup: EnvLookup,
    vault: Option<VaultClient>,
    aws: tokio::sync::OnceCell<aws_sdk_secretsmanager::Client>,
    k8s: Option<K8sClient>,
}

impl fmt::Debug for Engines {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Engines")
            .field("vault", &self.vault.is_some())
            .field("k8s", &self.k8s.is_some())
            .finish_non_exhaustive()
    }
}

impl Engines {
    #[must_use]
    pub fn new(vault: Option<VaultClient>, k8s: Option<K8sClient>) -> Self {
        Self {
            env_lookup: Box::new(|name| std::env::var(name).ok()),
            vault,
            aws: tokio::sync::OnceCell::new(),
            k8s,
        }
    }

    #[cfg(test)]
    fn with_env_lookup(mut self, lookup: EnvLookup) -> Self {
        self.env_lookup = lookup;
        self
    }

    async fn aws_client(&self) -> &aws_sdk_secretsmanager::Client {
        self.aws
            .get_or_init(|| async {
                let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
                aws_sdk_secretsmanager::Client::new(&config)
            })
            .await
    }

    async fn resolve_aws(
        &self,
        r: &SecretRef,
        name: &str,
        field: Option<&str>,
    ) -> Result<Credential, SecretError> {
        match self.aws_client().await.get_secret_value().secret_id(name).send().await {
            Ok(out) => {
                let Some(value) = out.secret_string().filter(|v| !v.is_empty()) else {
                    return Err(SecretError::not_found(r));
                };
                let Some(field) = field else { return Ok(Credential::new(value.to_owned())) };
                let json: serde_json::Value = serde_json::from_str(value).map_err(|e| {
                    SecretError::Backend(anyhow::anyhow!("aws-sm {name} is not JSON: {e}"))
                })?;
                match json.get(field).and_then(serde_json::Value::as_str) {
                    Some(v) if !v.is_empty() => Ok(Credential::new(v.to_owned())),
                    _ => Err(SecretError::not_found(r)),
                }
            }
            Err(e) if e.as_service_error().is_some_and(
                aws_sdk_secretsmanager::operation::get_secret_value::GetSecretValueError::is_resource_not_found_exception,
            ) => Err(SecretError::not_found(r)),
            Err(e) => Err(SecretError::Backend(anyhow::anyhow!(
                "aws secrets manager get {name}: {}",
                aws_sdk_secretsmanager::error::DisplayErrorContext(&e)
            ))),
        }
    }
}

#[async_trait::async_trait]
impl RefResolver for Engines {
    async fn resolve(&self, r: &SecretRef) -> Result<Credential, SecretError> {
        match r {
            SecretRef::Env { var } => match (self.env_lookup)(var) {
                Some(value) if !value.is_empty() => Ok(Credential::new(value)),
                _ => Err(SecretError::not_found(r)),
            },
            SecretRef::Vault { mount, path, field } => {
                let Some(vault) = &self.vault else {
                    return Err(SecretError::Backend(anyhow::anyhow!(
                        "vault engine not configured (need --vault-addr/--vault-role) for {r}"
                    )));
                };
                vault.read_kv2(r, mount, path, field).await
            }
            SecretRef::AwsSm { name, field } => self.resolve_aws(r, name, field.as_deref()).await,
            SecretRef::K8s { namespace, name, key } => {
                let Some(k8s) = &self.k8s else {
                    return Err(SecretError::Backend(anyhow::anyhow!(
                        "kubernetes engine not available (not in-cluster?) for {r}"
                    )));
                };
                k8s.read(r, namespace.as_deref(), name, key).await
            }
        }
    }
}

/// `HashiCorp` Vault / `OpenBao` KV v2 (Kubernetes auth via the pod SA token).
pub struct VaultClient {
    http: reqwest::Client,
    addr: String,
    role: String,
    token_path: PathBuf,
}

impl VaultClient {
    pub fn new(addr: String, role: String, token_path: PathBuf) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder().timeout(Duration::from_secs(10)).build()?;
        Ok(Self { http, addr, role, token_path })
    }

    fn backend_err(context: &str, e: impl fmt::Display) -> SecretError {
        SecretError::Backend(anyhow::anyhow!("{context}: {e}"))
    }

    async fn login(&self) -> Result<String, SecretError> {
        let jwt = tokio::fs::read_to_string(&self.token_path)
            .await
            .map_err(|e| Self::backend_err("reading service account token", e))?;
        let url = format!("{}/v1/auth/kubernetes/login", self.addr.trim_end_matches('/'));
        let resp = self
            .http
            .post(&url)
            .json(&serde_json::json!({ "jwt": jwt.trim(), "role": self.role }))
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

    async fn read_kv2(
        &self,
        r: &SecretRef,
        mount: &str,
        path: &str,
        field: &str,
    ) -> Result<Credential, SecretError> {
        let token = self.login().await?;
        let url = format!("{}/v1/{mount}/data/{path}", self.addr.trim_end_matches('/'));
        let resp = self
            .http
            .get(&url)
            .header("X-Vault-Token", &token)
            .send()
            .await
            .map_err(|e| Self::backend_err("vault read request", e))?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(SecretError::not_found(r));
        }
        if !status.is_success() {
            return Err(SecretError::Backend(anyhow::anyhow!("vault read failed: HTTP {status}")));
        }
        let body: serde_json::Value =
            resp.json().await.map_err(|e| Self::backend_err("vault read response", e))?;
        match body.pointer(&format!("/data/data/{field}")).and_then(serde_json::Value::as_str) {
            Some(value) if !value.is_empty() => Ok(Credential::new(value.to_owned())),
            _ => Err(SecretError::not_found(r)),
        }
    }
}

/// Kubernetes Secrets via the pod's own `ServiceAccount` (RBAC decides what it
/// may read).
pub struct K8sClient {
    http: reqwest::Client,
    api_base: String,
    token_path: PathBuf,
    default_namespace: String,
}

const K8S_SA_DIR: &str = "/var/run/secrets/kubernetes.io/serviceaccount";

impl K8sClient {
    /// Builds from the in-cluster environment (`KUBERNETES_SERVICE_HOST` + the
    /// projected SA dir). Errors off-cluster.
    pub fn in_cluster() -> anyhow::Result<Self> {
        let host = std::env::var("KUBERNETES_SERVICE_HOST")
            .map_err(|_| anyhow::anyhow!("KUBERNETES_SERVICE_HOST unset (not in-cluster)"))?;
        let port = std::env::var("KUBERNETES_SERVICE_PORT").unwrap_or_else(|_| "443".to_owned());
        let ca = std::fs::read(format!("{K8S_SA_DIR}/ca.crt"))?;
        let namespace = std::fs::read_to_string(format!("{K8S_SA_DIR}/namespace"))?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .add_root_certificate(reqwest::Certificate::from_pem(&ca)?)
            .build()?;
        Ok(Self {
            http,
            api_base: format!("https://{host}:{port}"),
            token_path: PathBuf::from(format!("{K8S_SA_DIR}/token")),
            default_namespace: namespace.trim().to_owned(),
        })
    }

    /// Test/off-cluster constructor with an explicit API base + token path.
    #[cfg(test)]
    pub fn with_base(api_base: String, token_path: PathBuf, default_namespace: String) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("reqwest client");
        Self { http, api_base, token_path, default_namespace }
    }

    async fn read(
        &self,
        r: &SecretRef,
        namespace: Option<&str>,
        name: &str,
        key: &str,
    ) -> Result<Credential, SecretError> {
        let err = |context: &str, e: &dyn fmt::Display| {
            SecretError::Backend(anyhow::anyhow!("k8s secret read: {context}: {e}"))
        };
        let token = tokio::fs::read_to_string(&self.token_path)
            .await
            .map_err(|e| err("reading service account token", &e))?;
        let ns = namespace.unwrap_or(&self.default_namespace);
        let url = format!(
            "{}/api/v1/namespaces/{ns}/secrets/{name}",
            self.api_base.trim_end_matches('/')
        );
        let resp = self
            .http
            .get(&url)
            .bearer_auth(token.trim())
            .send()
            .await
            .map_err(|e| err("request", &e))?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(SecretError::not_found(r));
        }
        if !status.is_success() {
            return Err(SecretError::Backend(anyhow::anyhow!(
                "k8s secret read failed: HTTP {status}"
            )));
        }
        let body: serde_json::Value = resp.json().await.map_err(|e| err("response", &e))?;
        let Some(b64) = body.pointer(&format!("/data/{key}")).and_then(serde_json::Value::as_str)
        else {
            return Err(SecretError::not_found(r));
        };
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| err("base64 decode", &e))?;
        match String::from_utf8(bytes) {
            Ok(value) if !value.is_empty() => Ok(Credential::new(value)),
            Ok(_) => Err(SecretError::not_found(r)),
            Err(e) => Err(err("utf-8 decode", &e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::response::IntoResponse;

    use super::*;

    fn env_ref(var: &str) -> SecretRef {
        SecretRef::Env { var: var.to_owned() }
    }

    struct MockResolver {
        response: StdMutex<Result<String, &'static str>>,
        calls: AtomicUsize,
    }

    impl MockResolver {
        fn ok(value: &str) -> Self {
            Self { response: StdMutex::new(Ok(value.to_owned())), calls: AtomicUsize::new(0) }
        }

        fn failing(msg: &'static str) -> Self {
            Self { response: StdMutex::new(Err(msg)), calls: AtomicUsize::new(0) }
        }
    }

    #[async_trait::async_trait]
    impl RefResolver for MockResolver {
        async fn resolve(&self, _r: &SecretRef) -> Result<Credential, SecretError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            match &*self.response.lock().unwrap() {
                Ok(v) => Ok(Credential::new(v.clone())),
                Err(msg) => Err(SecretError::Backend(anyhow::anyhow!(*msg))),
            }
        }
    }

    struct ArcResolver(std::sync::Arc<MockResolver>);

    #[async_trait::async_trait]
    impl RefResolver for ArcResolver {
        async fn resolve(&self, r: &SecretRef) -> Result<Credential, SecretError> {
            self.0.resolve(r).await
        }
    }

    #[tokio::test(start_paused = true)]
    async fn cache_serves_fresh_hit_without_engine_call() {
        let mock = std::sync::Arc::new(MockResolver::ok("s3cret"));
        let source =
            SecretSource::new(Box::new(ArcResolver(mock.clone())), Duration::from_secs(120));
        let r = env_ref("CRED_X");
        assert_eq!(source.fetch(&r).await.unwrap().expose(), "s3cret");
        tokio::time::advance(Duration::from_secs(119)).await;
        assert_eq!(source.fetch(&r).await.unwrap().expose(), "s3cret");
        assert_eq!(mock.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn cache_expiry_refetches_and_picks_up_rotation() {
        let mock = std::sync::Arc::new(MockResolver::ok("v1"));
        let source =
            SecretSource::new(Box::new(ArcResolver(mock.clone())), Duration::from_secs(120));
        let r = env_ref("CRED_X");
        assert_eq!(source.fetch(&r).await.unwrap().expose(), "v1");

        *mock.response.lock().unwrap() = Ok("v2".to_owned());
        tokio::time::advance(Duration::from_secs(119)).await;
        assert_eq!(
            source.fetch(&r).await.unwrap().expose(),
            "v1",
            "rotation must NOT land before TTL expiry"
        );
        assert_eq!(mock.calls.load(Ordering::SeqCst), 1);

        tokio::time::advance(Duration::from_secs(2)).await;
        assert_eq!(
            source.fetch(&r).await.unwrap().expose(),
            "v2",
            "rotation must land after TTL expiry"
        );
        assert_eq!(mock.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn engine_error_fails_closed_and_is_not_cached() {
        let mock = std::sync::Arc::new(MockResolver::failing("boom"));
        let source =
            SecretSource::new(Box::new(ArcResolver(mock.clone())), Duration::from_secs(120));
        let r = env_ref("CRED_X");
        assert!(matches!(source.fetch(&r).await, Err(SecretError::Backend(_))));
        *mock.response.lock().unwrap() = Ok("recovered".to_owned());
        assert_eq!(source.fetch(&r).await.unwrap().expose(), "recovered");
        assert_eq!(mock.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn engine_error_past_ttl_drops_stale_entry() {
        let mock = std::sync::Arc::new(MockResolver::ok("v1"));
        let source =
            SecretSource::new(Box::new(ArcResolver(mock.clone())), Duration::from_secs(120));
        let r = env_ref("CRED_X");
        assert_eq!(source.fetch(&r).await.unwrap().expose(), "v1");
        *mock.response.lock().unwrap() = Err("engine down");
        tokio::time::advance(Duration::from_secs(121)).await;
        assert!(matches!(source.fetch(&r).await, Err(SecretError::Backend(_))));
    }

    #[test]
    fn credential_debug_is_redacted() {
        let c = Credential::new("hunter2".to_owned());
        assert_eq!(format!("{c:?}"), "Credential(<redacted>)");
    }

    #[test]
    fn ref_parse_roundtrip() {
        for (input, display) in [
            ("env:GITHUB_TOKEN", "env:GITHUB_TOKEN"),
            (
                "vault:kvmount/data/cctui/workers#GITHUB_TOKEN_ACME",
                "vault:kvmount/data/cctui/workers#GITHUB_TOKEN_ACME",
            ),
            ("vault:kv/data/a/b/c#f", "vault:kv/data/a/b/c#f"),
            ("aws-sm:cctui/worker/acme/github", "aws-sm:cctui/worker/acme/github"),
            ("aws-sm:cctui/worker#token", "aws-sm:cctui/worker#token"),
            ("k8s:my-secret#token", "k8s:my-secret#token"),
            ("k8s:dev/my-secret#token", "k8s:dev/my-secret#token"),
            ("bao:kvmount/data/cctui/workers#GPG_KEY", "vault:kvmount/data/cctui/workers#GPG_KEY"),
        ] {
            let r = SecretRef::parse(input).unwrap();
            assert_eq!(r.to_string(), display, "roundtrip of {input}");
            assert_eq!(SecretRef::parse(&r.to_string()).unwrap(), r);
        }
    }

    #[test]
    fn ref_parse_rejects_malformed() {
        for bad in [
            "",
            "GITHUB_TOKEN",
            "env:",
            "vault:kvmount#F",
            "vault:kvmount/data/x",
            "vault:/data/x#f",
            "vault:m/data/#f",
            "vault:m/data/p#",
            "aws-sm:",
            "aws-sm:name#",
            "k8s:secret",
            "k8s:#key",
            "k8s:/secret#key",
            "s3:bucket/key",
        ] {
            assert!(SecretRef::parse(bad).is_err(), "{bad:?} must be rejected");
        }
    }

    #[test]
    fn identity_templating() {
        assert_eq!(
            render_identity("vault:c/data/w#GITHUB_TOKEN_${IDENTITY}", "acme").unwrap(),
            "vault:c/data/w#GITHUB_TOKEN_ACME"
        );
        assert_eq!(
            render_identity("vault:c/data/w/${identity}#token", "acme-corp").unwrap(),
            "vault:c/data/w/acme-corp#token"
        );
        assert_eq!(
            render_identity("env:CRED_${IDENTITY}_GH", "acme-corp").unwrap(),
            "env:CRED_ACME_CORP_GH"
        );
        // No placeholder: identity irrelevant, even empty.
        assert_eq!(render_identity("env:STATIC", "").unwrap(), "env:STATIC");
        // Placeholder with no identity must error, not resolve to a wrong path.
        assert!(render_identity("env:CRED_${IDENTITY}_GH", "").is_err());
        // Unknown placeholder must error rather than pass through.
        assert!(render_identity("env:CRED_${TYPO}", "acme").is_err());
    }

    #[test]
    fn legacy_derivations_match_old_conventions() {
        assert_eq!(
            SecretRef::legacy_env("acme-corp", "github"),
            SecretRef::parse("env:CRED_ACME_CORP_GITHUB").unwrap()
        );
        assert_eq!(
            SecretRef::legacy_vault("kvmount", "cctui/workers", "value", "acme", "github"),
            SecretRef::parse("vault:kvmount/data/cctui/workers/acme/github#value").unwrap()
        );
        assert_eq!(
            SecretRef::legacy_vault("kvmount", "", "value", "acme", "github"),
            SecretRef::parse("vault:kvmount/data/acme/github#value").unwrap()
        );
        assert_eq!(
            SecretRef::legacy_aws_sm("cctui/worker", "acme", "github"),
            SecretRef::parse("aws-sm:cctui/worker/acme/github").unwrap()
        );
    }

    #[tokio::test]
    async fn env_engine_resolves_and_fails_closed() {
        let engines = Engines::new(None, None).with_env_lookup(Box::new(|name| match name {
            "CRED_ACME_GITHUB" => Some("tok".to_owned()),
            "CRED_ACME_EMPTY" => Some(String::new()),
            _ => None,
        }));
        assert_eq!(engines.resolve(&env_ref("CRED_ACME_GITHUB")).await.unwrap().expose(), "tok");
        assert!(matches!(
            engines.resolve(&env_ref("CRED_ACME_EMPTY")).await,
            Err(SecretError::NotFound { .. })
        ));
        assert!(matches!(
            engines.resolve(&env_ref("CRED_ACME_MISSING")).await,
            Err(SecretError::NotFound { .. })
        ));
    }

    #[tokio::test]
    async fn unconfigured_engines_fail_closed_as_backend() {
        let engines = Engines::new(None, None);
        let vault = SecretRef::parse("vault:m/data/p#f").unwrap();
        assert!(matches!(engines.resolve(&vault).await, Err(SecretError::Backend(_))));
        let k8s = SecretRef::parse("k8s:sec#key").unwrap();
        assert!(matches!(engines.resolve(&k8s).await, Err(SecretError::Backend(_))));
    }

    async fn spawn_vault_mock() -> String {
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
                "/v1/kvmount/data/dev/cctui-worker",
                get(|headers: HeaderMap| async move {
                    if headers.get("X-Vault-Token").and_then(|v| v.to_str().ok()) != Some("tok-123")
                    {
                        return (StatusCode::FORBIDDEN, "bad token").into_response();
                    }
                    Json(serde_json::json!({"data": {"data": {
                        "GITHUB_TOKEN_ACME": "hunter2",
                        "EMPTY": "",
                    }}}))
                    .into_response()
                }),
            )
            .route(
                "/v1/kvmount/data/boom",
                get(|| async { (StatusCode::INTERNAL_SERVER_ERROR, "vault sad").into_response() }),
            );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    }

    fn write_token(dir: &tempfile::TempDir, contents: &str) -> PathBuf {
        let path = dir.path().join("token");
        std::fs::write(&path, contents).unwrap();
        path
    }

    fn vault_engines(addr: String, token_path: PathBuf) -> Engines {
        let client = VaultClient::new(addr, "cctui-worker".to_owned(), token_path).unwrap();
        Engines::new(Some(client), None)
    }

    #[tokio::test]
    async fn vault_engine_logs_in_and_reads_kv2_field() {
        let addr = spawn_vault_mock().await;
        let dir = tempfile::tempdir().unwrap();
        let engines = vault_engines(addr, write_token(&dir, "test-jwt\n"));
        let r = SecretRef::parse("vault:kvmount/data/dev/cctui-worker#GITHUB_TOKEN_ACME").unwrap();
        assert_eq!(engines.resolve(&r).await.unwrap().expose(), "hunter2");
    }

    #[tokio::test]
    async fn vault_engine_missing_path_field_or_empty_is_not_found() {
        let addr = spawn_vault_mock().await;
        let dir = tempfile::tempdir().unwrap();
        let engines = vault_engines(addr, write_token(&dir, "test-jwt"));
        for r in [
            "vault:kvmount/data/nope#GITHUB_TOKEN_ACME",
            "vault:kvmount/data/dev/cctui-worker#MISSING_FIELD",
            "vault:kvmount/data/dev/cctui-worker#EMPTY",
        ] {
            let r = SecretRef::parse(r).unwrap();
            assert!(
                matches!(engines.resolve(&r).await, Err(SecretError::NotFound { .. })),
                "{r} must be NotFound"
            );
        }
    }

    #[tokio::test]
    async fn vault_engine_server_error_and_bad_login_fail_closed() {
        let addr = spawn_vault_mock().await;
        let dir = tempfile::tempdir().unwrap();
        let engines = vault_engines(addr.clone(), write_token(&dir, "test-jwt"));
        let r = SecretRef::parse("vault:kvmount/data/boom#f").unwrap();
        assert!(matches!(engines.resolve(&r).await, Err(SecretError::Backend(_))));

        let engines = vault_engines(addr, write_token(&dir, "wrong-jwt"));
        let r = SecretRef::parse("vault:kvmount/data/dev/cctui-worker#GITHUB_TOKEN_ACME").unwrap();
        assert!(matches!(engines.resolve(&r).await, Err(SecretError::Backend(_))));
    }

    #[tokio::test]
    async fn vault_engine_unreadable_token_fails_closed() {
        let addr = spawn_vault_mock().await;
        let engines = vault_engines(addr, PathBuf::from("/nonexistent/token"));
        let r = SecretRef::parse("vault:kvmount/data/dev/cctui-worker#GITHUB_TOKEN_ACME").unwrap();
        assert!(matches!(engines.resolve(&r).await, Err(SecretError::Backend(_))));
    }

    async fn spawn_k8s_mock() -> String {
        use axum::extract::Path;
        use axum::http::{HeaderMap, StatusCode};
        use axum::routing::get;
        use axum::{Json, Router};

        let app = Router::new().route(
            "/api/v1/namespaces/{ns}/secrets/{name}",
            get(|Path((ns, name)): Path<(String, String)>, headers: HeaderMap| async move {
                if headers.get("authorization").and_then(|v| v.to_str().ok())
                    != Some("Bearer sa-token")
                {
                    return (StatusCode::UNAUTHORIZED, "bad token").into_response();
                }
                match (ns.as_str(), name.as_str()) {
                    ("dev", "worker-creds") => Json(serde_json::json!({"data": {
                        "token": base64::engine::general_purpose::STANDARD.encode("k8s-tok"),
                    }}))
                    .into_response(),
                    _ => (StatusCode::NOT_FOUND, "no secret").into_response(),
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn k8s_engine_reads_secret_key_with_ns_default_and_override() {
        let base = spawn_k8s_mock().await;
        let dir = tempfile::tempdir().unwrap();
        let token = write_token(&dir, "sa-token\n");
        let engines = Engines::new(None, Some(K8sClient::with_base(base, token, "dev".to_owned())));

        let by_default_ns = SecretRef::parse("k8s:worker-creds#token").unwrap();
        assert_eq!(engines.resolve(&by_default_ns).await.unwrap().expose(), "k8s-tok");
        let by_explicit_ns = SecretRef::parse("k8s:dev/worker-creds#token").unwrap();
        assert_eq!(engines.resolve(&by_explicit_ns).await.unwrap().expose(), "k8s-tok");

        let missing_secret = SecretRef::parse("k8s:nope#token").unwrap();
        assert!(matches!(
            engines.resolve(&missing_secret).await,
            Err(SecretError::NotFound { .. })
        ));
        let missing_key = SecretRef::parse("k8s:worker-creds#missing").unwrap();
        assert!(matches!(engines.resolve(&missing_key).await, Err(SecretError::NotFound { .. })));
        let wrong_ns = SecretRef::parse("k8s:prod/worker-creds#token").unwrap();
        assert!(matches!(engines.resolve(&wrong_ns).await, Err(SecretError::NotFound { .. })));
    }

    #[tokio::test]
    async fn k8s_engine_bad_sa_token_fails_closed() {
        let base = spawn_k8s_mock().await;
        let dir = tempfile::tempdir().unwrap();
        let engines = Engines::new(
            None,
            Some(K8sClient::with_base(base, write_token(&dir, "wrong"), "dev".to_owned())),
        );
        let r = SecretRef::parse("k8s:worker-creds#token").unwrap();
        assert!(matches!(engines.resolve(&r).await, Err(SecretError::Backend(_))));
    }
}
