//! Pluggable secret resolution. Every secret is named by an explicit
//! [`SecretRef`] — engines (env / file / Vault / AWS SM / Kubernetes secrets),
//! never a site-specific taxonomy. Invariants the types cannot express:
//! secrets stay in memory only (TTL cache, never persisted), `Credential`
//! redacts `Debug` so values never reach logs, and an engine failure is
//! `SecretError::Backend` — distinct from `NotFound` — so callers fail closed
//! instead of using a blank secret. A fetch past TTL re-reads the engine, so
//! store-side rotation lands within one TTL.
//!
//! Config values go through [`SecretValue`]: a value starting with one of
//! [`SCHEMES`] is a ref, anything else is a literal, and a `literal:` prefix
//! forces the rest to be taken verbatim.
//!
//! The `bao`, `k8s` and `aws-sm` engines are cargo features; a ref to an engine
//! that is not compiled in fails as `Backend`. The HTTP engines expect the
//! process to have installed a rustls `CryptoProvider`.

#[cfg(feature = "aws-sm")]
mod aws;
#[cfg(feature = "bao")]
mod bao;
#[cfg(feature = "k8s")]
mod k8s;

use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

use tokio::sync::Mutex;
use tokio::time::Instant;

#[cfg(feature = "bao")]
pub use bao::VaultClient;
#[cfg(feature = "k8s")]
pub use k8s::K8sClient;

/// Prefixes that make a config value a [`SecretRef`].
pub const SCHEMES: [&str; 6] = ["env:", "file:", "vault:", "bao:", "aws-sm:", "k8s:"];

/// Escape for a literal config value that would otherwise parse as a ref.
pub const LITERAL_PREFIX: &str = "literal:";

/// A secret value. `Debug` is redacted; use [`Credential::expose`] at the
/// point of use only.
#[derive(Clone)]
pub struct Credential {
    value: String,
}

impl Credential {
    #[must_use]
    pub const fn new(value: String) -> Self {
        Self { value }
    }

    #[must_use]
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
    /// The engine failed. The caller must fail closed — never use a blank or
    /// stale-beyond-TTL secret.
    #[error("secret backend failure: {0}")]
    Backend(anyhow::Error),
}

impl SecretError {
    pub(crate) fn not_found(r: &SecretRef) -> Self {
        Self::NotFound { what: r.to_string() }
    }

    #[cfg(any(not(feature = "bao"), not(feature = "k8s"), not(feature = "aws-sm")))]
    fn not_compiled(r: &SecretRef, feature: &str) -> Self {
        Self::Backend(anyhow::anyhow!(
            "{r}: engine not compiled in (cctui-secrets feature `{feature}`)"
        ))
    }
}

/// An explicit, engine-qualified secret address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretRef {
    /// `env:VAR` — the process's own environment.
    Env { var: String },
    /// `file:<path>` — a file's contents, one trailing newline stripped.
    File { path: String },
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
    /// Parses the canonical string form (see variant docs).
    pub fn parse(s: &str) -> anyhow::Result<Self> {
        let s = s.trim();
        if let Some(var) = s.strip_prefix("env:") {
            anyhow::ensure!(!var.is_empty(), "empty env ref");
            return Ok(Self::Env { var: var.to_owned() });
        }
        if let Some(path) = s.strip_prefix("file:") {
            anyhow::ensure!(!path.is_empty(), "empty file ref");
            return Ok(Self::File { path: path.to_owned() });
        }
        // `bao:` aliases `vault:` for refs carried in POD ENV VALUES: the
        // bank-vaults webhook mutates any env value with a literal `vault:`
        // prefix, which would hijack the ref before it is ever read.
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
        anyhow::bail!(
            "unknown secret ref scheme in {s:?} (expected env:/file:/vault:/bao:/aws-sm:/k8s:)"
        )
    }
}

impl fmt::Display for SecretRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Env { var } => write!(f, "env:{var}"),
            Self::File { path } => write!(f, "file:{path}"),
            Self::Vault { mount, path, field } => write!(f, "vault:{mount}/data/{path}#{field}"),
            Self::AwsSm { name, field: None } => write!(f, "aws-sm:{name}"),
            Self::AwsSm { name, field: Some(field) } => write!(f, "aws-sm:{name}#{field}"),
            Self::K8s { namespace: None, name, key } => write!(f, "k8s:{name}#{key}"),
            Self::K8s { namespace: Some(ns), name, key } => write!(f, "k8s:{ns}/{name}#{key}"),
        }
    }
}

/// A config value: either used as written, or a ref to resolve.
#[derive(Debug, PartialEq, Eq)]
pub enum SecretValue<'a> {
    Literal(&'a str),
    Ref(SecretRef),
}

impl<'a> SecretValue<'a> {
    /// Only the exact [`SCHEMES`] make a ref, so `postgres://u:p@h/db` or a
    /// token containing `:` stay literals. A malformed ref is an error, not a
    /// silent literal.
    pub fn parse(raw: &'a str) -> anyhow::Result<Self> {
        if let Some(literal) = raw.strip_prefix(LITERAL_PREFIX) {
            return Ok(Self::Literal(literal));
        }
        let head = raw.trim_start();
        if SCHEMES.iter().any(|scheme| head.starts_with(scheme)) {
            return SecretRef::parse(raw).map(Self::Ref);
        }
        Ok(Self::Literal(raw))
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
    #[must_use]
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

pub type EnvLookup = Box<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// All engines behind one resolver.
///
/// A ref names its engine, so one process serves `env:` and `vault:` refs side
/// by side. An engine that is not configured fails its refs as `Backend`.
pub struct Engines {
    env_lookup: EnvLookup,
    #[cfg(feature = "bao")]
    vault: Option<VaultClient>,
    #[cfg(feature = "aws-sm")]
    aws: tokio::sync::OnceCell<aws_sdk_secretsmanager::Client>,
    #[cfg(feature = "k8s")]
    k8s: Option<K8sClient>,
}

impl fmt::Debug for Engines {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut d = f.debug_struct("Engines");
        #[cfg(feature = "bao")]
        d.field("vault", &self.vault.is_some());
        #[cfg(feature = "k8s")]
        d.field("k8s", &self.k8s.is_some());
        d.finish_non_exhaustive()
    }
}

impl Default for Engines {
    fn default() -> Self {
        Self::new()
    }
}

impl Engines {
    /// `env:` and `file:` only; add the others with the `with_*` builders.
    #[must_use]
    pub fn new() -> Self {
        Self {
            env_lookup: Box::new(|name| std::env::var(name).ok()),
            #[cfg(feature = "bao")]
            vault: None,
            #[cfg(feature = "aws-sm")]
            aws: tokio::sync::OnceCell::new(),
            #[cfg(feature = "k8s")]
            k8s: None,
        }
    }

    /// The engine set for a process resolving its own config: the `vault:`
    /// engine from `CCTUI_SECRETS_VAULT_ADDR` + `CCTUI_SECRETS_VAULT_ROLE`
    /// (+ optional `CCTUI_SECRETS_VAULT_TOKEN_PATH`), the `k8s:` engine when
    /// in-cluster, `aws-sm:` from ambient AWS config.
    #[allow(clippy::unnecessary_wraps)]
    pub fn from_env() -> anyhow::Result<Self> {
        let engines = Self::new();
        #[cfg(feature = "bao")]
        let engines = {
            let get = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
            let vault = match (get("CCTUI_SECRETS_VAULT_ADDR"), get("CCTUI_SECRETS_VAULT_ROLE")) {
                (Some(addr), Some(role)) => {
                    let token_path = get("CCTUI_SECRETS_VAULT_TOKEN_PATH")
                        .map_or_else(|| bao::DEFAULT_TOKEN_PATH.into(), Into::into);
                    Some(VaultClient::new(addr, role, token_path)?)
                }
                (Some(_), None) => anyhow::bail!(
                    "CCTUI_SECRETS_VAULT_ROLE is required with CCTUI_SECRETS_VAULT_ADDR"
                ),
                (None, _) => None,
            };
            engines.with_vault(vault)
        };
        #[cfg(feature = "k8s")]
        let engines = engines.with_k8s(K8sClient::in_cluster().ok());
        Ok(engines)
    }

    #[must_use]
    pub fn with_env_lookup(mut self, lookup: EnvLookup) -> Self {
        self.env_lookup = lookup;
        self
    }

    #[cfg(feature = "bao")]
    #[must_use]
    pub fn with_vault(mut self, vault: Option<VaultClient>) -> Self {
        self.vault = vault;
        self
    }

    #[cfg(feature = "k8s")]
    #[must_use]
    pub fn with_k8s(mut self, k8s: Option<K8sClient>) -> Self {
        self.k8s = k8s;
        self
    }

    /// Pins the Secrets Manager client instead of loading ambient AWS config
    /// on first use.
    #[cfg(feature = "aws-sm")]
    #[must_use]
    pub fn with_aws_client(mut self, client: aws_sdk_secretsmanager::Client) -> Self {
        self.aws = tokio::sync::OnceCell::new_with(Some(client));
        self
    }

    #[cfg(feature = "aws-sm")]
    async fn aws_client(&self) -> &aws_sdk_secretsmanager::Client {
        self.aws
            .get_or_init(|| async {
                let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
                aws_sdk_secretsmanager::Client::new(&config)
            })
            .await
    }

    #[cfg_attr(not(feature = "bao"), allow(clippy::unused_async))]
    async fn resolve_vault(
        &self,
        r: &SecretRef,
        mount: &str,
        path: &str,
        field: &str,
    ) -> Result<Credential, SecretError> {
        #[cfg(feature = "bao")]
        {
            let Some(vault) = &self.vault else {
                return Err(SecretError::Backend(anyhow::anyhow!(
                    "vault engine not configured (need a vault address and role) for {r}"
                )));
            };
            vault.read_kv2(r, mount, path, field).await
        }
        #[cfg(not(feature = "bao"))]
        {
            let _ = (mount, path, field);
            Err(SecretError::not_compiled(r, "bao"))
        }
    }

    #[cfg_attr(not(feature = "aws-sm"), allow(clippy::unused_async))]
    async fn resolve_aws(
        &self,
        r: &SecretRef,
        name: &str,
        field: Option<&str>,
    ) -> Result<Credential, SecretError> {
        #[cfg(feature = "aws-sm")]
        {
            aws::get(self.aws_client().await, r, name, field).await
        }
        #[cfg(not(feature = "aws-sm"))]
        {
            let _ = (name, field);
            Err(SecretError::not_compiled(r, "aws-sm"))
        }
    }

    #[cfg_attr(not(feature = "k8s"), allow(clippy::unused_async))]
    async fn resolve_k8s(
        &self,
        r: &SecretRef,
        namespace: Option<&str>,
        name: &str,
        key: &str,
    ) -> Result<Credential, SecretError> {
        #[cfg(feature = "k8s")]
        {
            let Some(k8s) = &self.k8s else {
                return Err(SecretError::Backend(anyhow::anyhow!(
                    "kubernetes engine not available (not in-cluster?) for {r}"
                )));
            };
            k8s.read(r, namespace, name, key).await
        }
        #[cfg(not(feature = "k8s"))]
        {
            let _ = (namespace, name, key);
            Err(SecretError::not_compiled(r, "k8s"))
        }
    }
}

async fn read_file(r: &SecretRef, path: &str) -> Result<Credential, SecretError> {
    match tokio::fs::read_to_string(path).await {
        Ok(raw) => {
            let value =
                raw.strip_suffix('\n').map_or(raw.as_str(), |v| v.strip_suffix('\r').unwrap_or(v));
            if value.is_empty() {
                Err(SecretError::not_found(r))
            } else {
                Ok(Credential::new(value.to_owned()))
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(SecretError::not_found(r)),
        Err(e) => Err(SecretError::Backend(anyhow::anyhow!("reading {r}: {e}"))),
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
            SecretRef::File { path } => read_file(r, path).await,
            SecretRef::Vault { mount, path, field } => {
                self.resolve_vault(r, mount, path, field).await
            }
            SecretRef::AwsSm { name, field } => self.resolve_aws(r, name, field.as_deref()).await,
            SecretRef::K8s { namespace, name, key } => {
                self.resolve_k8s(r, namespace.as_deref(), name, key).await
            }
        }
    }
}

/// Resolves one config value against `resolver`: literals pass through, refs
/// are fetched. Errors name the ref, never the value.
pub async fn resolve_value(resolver: &dyn RefResolver, raw: &str) -> anyhow::Result<String> {
    match SecretValue::parse(raw)? {
        SecretValue::Literal(value) => Ok(value.to_owned()),
        SecretValue::Ref(r) => resolve_ref(resolver, &r).await,
    }
}

async fn resolve_ref(resolver: &dyn RefResolver, r: &SecretRef) -> anyhow::Result<String> {
    resolver.resolve(r).await.map(|c| c.value).map_err(|e| anyhow::anyhow!("secret ref {r}: {e}"))
}

/// Like [`resolve_value`] with [`Engines::from_env`], built only when a ref
/// actually needs it.
async fn resolve_lazily(engines: &mut Option<Engines>, raw: &str) -> anyhow::Result<String> {
    let r = match SecretValue::parse(raw)? {
        SecretValue::Literal(value) => return Ok(value.to_owned()),
        SecretValue::Ref(r) => r,
    };
    if engines.is_none() {
        *engines = Some(Engines::from_env()?);
    }
    let Some(engines) = engines.as_ref() else { anyhow::bail!("no secret engines for {r}") };
    resolve_ref(engines, &r).await
}

async fn resolve_pairs(pairs: Vec<(String, String)>) -> anyhow::Result<HashMap<String, String>> {
    let mut engines = None;
    let mut out = HashMap::with_capacity(pairs.len());
    for (name, raw) in pairs {
        let value =
            resolve_lazily(&mut engines, &raw).await.map_err(|e| anyhow::anyhow!("{name}: {e}"))?;
        out.insert(name, value);
    }
    Ok(out)
}

fn lookup_pairs(names: &[&str], get: impl Fn(&str) -> Option<String>) -> Vec<(String, String)> {
    names.iter().filter_map(|name| get(name).map(|raw| ((*name).to_owned(), raw))).collect()
}

/// Resolves each of `names` that `get` returns, keyed by name. Unset names are
/// absent from the result.
pub async fn resolve_env(
    names: &[&str],
    get: impl Fn(&str) -> Option<String>,
) -> anyhow::Result<HashMap<String, String>> {
    resolve_pairs(lookup_pairs(names, get)).await
}

/// [`resolve_env`] for sync callers; safe inside or outside a tokio runtime.
pub fn resolve_env_blocking(
    names: &[&str],
    get: impl Fn(&str) -> Option<String>,
) -> anyhow::Result<HashMap<String, String>> {
    block_on(resolve_pairs(lookup_pairs(names, get)))
}

/// One config value for sync callers; safe inside or outside a tokio runtime.
pub fn resolve_blocking(raw: &str) -> anyhow::Result<String> {
    if let SecretValue::Literal(value) = SecretValue::parse(raw)? {
        return Ok(value.to_owned());
    }
    block_on(async { resolve_lazily(&mut None, raw).await })
}

/// Runs `fut` on a private current-thread runtime in a scoped thread, so it
/// never nests inside the caller's runtime.
fn block_on<T: Send>(fut: impl Future<Output = anyhow::Result<T>> + Send) -> anyhow::Result<T> {
    std::thread::scope(|s| {
        s.spawn(move || {
            tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(fut)
        })
        .join()
    })
    .map_err(|_| anyhow::anyhow!("secret resolver thread panicked"))?
}

#[cfg(test)]
mod tests;
