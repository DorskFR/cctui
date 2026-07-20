//! TLS-terminating credential injection (CCT-718): the strip-then-substitute
//! (phantom-token) pattern.
//!
//! For a host on the *injection* allow-list the proxy terminates TLS (presenting
//! a leaf cert minted on the fly and signed by a per-pod CA), parses each
//! HTTP/1.1 request, STRIPS whatever credential the agent supplied
//! (`Authorization` / npm `_authToken` bearer / git Basic / session cookie),
//! looks up the REAL credential by the rule's explicit [`SecretRef`] via the
//! [`SecretSource`], substitutes it, and forwards upstream over real TLS
//! (validating the upstream cert against the public roots). Hosts NOT on the
//! allow-list keep the SNI-peek passthrough splice in `transparent.rs`, so the
//! MITM surface stays minimal.
//!
//! Fail-closed nuance (per the ticket): the agent never holds a real secret, so
//! a lookup miss or backend error forwards the agent's ORIGINAL request head
//! UNCHANGED (the upstream rejects the placeholder) — never a blank/wrong
//! secret. Only a successful fetch triggers the strip-and-substitute.
//!
//! Cert-PINNING services are deliberately absent from [`builtin_rule`]: a CLI
//! that pins its server cert would break under this MITM. Such hosts must stay
//! passthrough (never listed as an inject host). None of the built-in hosts pin
//! certs on their HTTPS REST/registry endpoints.

#![allow(clippy::large_futures)]

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use base64::Engine as _;
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose,
};
use rustls::crypto::aws_lc_rs::sign::any_supported_type;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::{TlsAcceptor, TlsConnector};

use crate::ghapp::GhAppMinter;
use crate::secrets::{Credential, SecretError, SecretRef, SecretSource, render_identity};

/// How to shape the injected credential for a given host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthShape {
    /// `Authorization: Bearer <cred>` — GitHub API, npm registry, Sentry,
    /// `YouTrack`, Figma, most REST APIs. npm's on-disk `_authToken` is sent as a
    /// bearer header, so this covers it too.
    Bearer,
    /// `Authorization: Basic base64(<username>:<cred>)` — git-over-HTTPS. GitHub
    /// accepts any username with a token as the password; the convention is
    /// `x-access-token`.
    Basic { username: String },
    /// `Authorization: Bearer <cred>` plus a `<cookie_name>=<value>` cookie from
    /// the rule's companion `cookie_secret` — e.g. Slack's browser (`xoxc`)
    /// session tokens need their `d` cookie.
    BearerCookie { cookie_name: String },
}

/// One allow-listed host and how to inject its credential. The secret is an
/// explicit engine-qualified [`SecretRef`] — the proxy carries no service
/// taxonomy of its own.
#[derive(Debug, Clone)]
pub struct InjectionRule {
    pub host: String,
    /// `None` matches every path; the longest matching prefix wins per request.
    pub path_prefix: Option<String>,
    /// Label for logs and provider matching (the GitHub-App minter), not a
    /// lookup key.
    pub service: String,
    pub shape: AuthShape,
    pub secret: SecretRef,
    /// Companion cookie value; required by [`AuthShape::BearerCookie`].
    pub cookie_secret: Option<SecretRef>,
}

/// A `host → service/shape` mapping before a secret ref is attached (the legacy
/// `--inject-hosts` path derives refs from the configured backend).
#[derive(Debug, Clone)]
pub struct HostSpec {
    pub host: String,
    pub service: String,
    pub shape: AuthShape,
}

/// Convenience `host → (service, shape)` table for the legacy `--inject-hosts`
/// form. `host=service:<shape>` overrides; the JSON inject config bypasses this
/// entirely.
#[must_use]
pub fn builtin_rule(host: &str) -> Option<HostSpec> {
    let (service, shape) = match host {
        "api.github.com" => ("github", AuthShape::Bearer),
        // git smart-HTTP clone/fetch/push send Basic auth; rewrite to the
        // GitHub-blessed `x-access-token:<token>` Basic form.
        "github.com" => ("github", AuthShape::Basic { username: "x-access-token".to_owned() }),
        // npm sends the registry `_authToken` as `Authorization: Bearer`.
        "registry.npmjs.org" => ("npm", AuthShape::Bearer),
        "slack.com" | "api.slack.com" => {
            ("slack", AuthShape::BearerCookie { cookie_name: "d".to_owned() })
        }
        "api.figma.com" => ("figma", AuthShape::Bearer),
        _ => return None,
    };
    Some(HostSpec { host: host.to_owned(), service: service.to_owned(), shape })
}

/// Parses one `--inject-hosts` token into a [`HostSpec`].
///
/// Accepted forms:
/// - `host` — use the built-in mapping; unknown hosts fall back to Bearer with
///   the host as the service label.
/// - `host=service` — Bearer with an explicit service name.
/// - `host=service:git` / `:basic` / `:bearer` / `:cookie` — explicit shape.
///
/// The host is lowercased; an empty token yields `None`.
#[must_use]
pub fn parse_inject_host(token: &str) -> Option<HostSpec> {
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    let (host, spec) = token.split_once('=').map_or((token, None), |(h, s)| (h, Some(s)));
    let host = host.trim().to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    let Some(spec) = spec else {
        return Some(builtin_rule(&host).unwrap_or_else(|| HostSpec {
            host: host.clone(),
            service: host,
            shape: AuthShape::Bearer,
        }));
    };
    let (service, shape_str) = spec.split_once(':').map_or((spec, "bearer"), |(s, st)| (s, st));
    let shape = match shape_str.trim().to_ascii_lowercase().as_str() {
        "git" | "gitbasic" | "git-basic" | "basic" => {
            AuthShape::Basic { username: "x-access-token".to_owned() }
        }
        "slack" | "cookie" | "bearer+cookie" => {
            AuthShape::BearerCookie { cookie_name: "d".to_owned() }
        }
        _ => AuthShape::Bearer,
    };
    Some(HostSpec { host, service: service.trim().to_owned(), shape })
}

/// One entry of the JSON inject config: which host, what auth shape, and the
/// explicit secret ref(s) to inject. `${IDENTITY}`/`${identity}` templating
/// applies to the refs.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct InjectEntry {
    host: String,
    #[serde(default)]
    path_prefix: Option<String>,
    #[serde(default)]
    service: Option<String>,
    /// `bearer` (default) | `basic`/`git` | `bearer+cookie`/`cookie`.
    #[serde(default)]
    shape: Option<String>,
    /// Basic-auth username; default `x-access-token`.
    #[serde(default)]
    username: Option<String>,
    /// Cookie name for `bearer+cookie`; default `d`.
    #[serde(default)]
    cookie_name: Option<String>,
    secret: String,
    #[serde(default)]
    cookie_secret: Option<String>,
}

impl InjectEntry {
    fn into_rule(self, identity: &str) -> anyhow::Result<InjectionRule> {
        let host = self.host.trim().to_ascii_lowercase();
        anyhow::ensure!(!host.is_empty(), "inject entry with an empty host");
        let shape =
            match self.shape.as_deref().unwrap_or("bearer").trim().to_ascii_lowercase().as_str() {
                "bearer" => AuthShape::Bearer,
                "basic" | "git" | "gitbasic" | "git-basic" => AuthShape::Basic {
                    username: self.username.unwrap_or_else(|| "x-access-token".to_owned()),
                },
                "bearer+cookie" | "cookie" | "slack" => AuthShape::BearerCookie {
                    cookie_name: self.cookie_name.unwrap_or_else(|| "d".to_owned()),
                },
                other => anyhow::bail!("inject entry for {host}: unknown shape {other:?}"),
            };
        let secret = SecretRef::parse(&render_identity(&self.secret, identity)?)?;
        let cookie_secret = self
            .cookie_secret
            .map(|s| SecretRef::parse(&render_identity(&s, identity)?))
            .transpose()?;
        anyhow::ensure!(
            !matches!(shape, AuthShape::BearerCookie { .. }) || cookie_secret.is_some(),
            "inject entry for {host}: bearer+cookie shape needs cookie_secret"
        );
        let path_prefix = self
            .path_prefix
            .map(|p| {
                let p = p.trim().to_owned();
                anyhow::ensure!(
                    p.starts_with('/'),
                    "inject entry for {host}: path_prefix must start with '/'"
                );
                Ok(p)
            })
            .transpose()?;
        Ok(InjectionRule {
            service: self.service.unwrap_or_else(|| host.clone()),
            host,
            path_prefix,
            shape,
            secret,
            cookie_secret,
        })
    }
}

/// Parses the JSON inject config (an array of entries) into rules, applying
/// identity templating to every secret ref. Any malformed entry fails the whole
/// load — a partially applied credential config is worse than none.
pub fn load_inject_config(json: &str, identity: &str) -> anyhow::Result<Vec<InjectionRule>> {
    let entries: Vec<InjectEntry> =
        serde_json::from_str(json).map_err(|e| anyhow::anyhow!("parsing inject config: {e}"))?;
    entries.into_iter().map(|e| e.into_rule(identity)).collect()
}

/// The injection allow-list. Hosts NOT listed here keep today's SNI-peek
/// passthrough (no TLS termination).
#[derive(Debug, Clone)]
pub struct InjectionPolicy {
    rules: Vec<InjectionRule>,
    by_host: HashMap<String, Vec<usize>>,
}

impl InjectionPolicy {
    #[must_use]
    pub fn new(rules: Vec<InjectionRule>) -> Self {
        let mut by_host: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, r) in rules.iter().enumerate() {
            by_host.entry(r.host.to_ascii_lowercase()).or_default().push(i);
        }
        Self { rules, by_host }
    }

    /// Rules for `host` (case-insensitive) in config order; empty ⇒ passthrough.
    #[must_use]
    pub fn rules_for(&self, host: &str) -> Vec<&InjectionRule> {
        self.by_host
            .get(&host.to_ascii_lowercase())
            .map(|idxs| idxs.iter().map(|&i| &self.rules[i]).collect())
            .unwrap_or_default()
    }
}

/// Longest matching `path_prefix` wins; ties keep config order.
fn select_rule<'a>(rules: &'a [InjectionRule], path: &str) -> Option<&'a InjectionRule> {
    let mut best: Option<&InjectionRule> = None;
    for r in rules {
        if r.path_prefix.as_deref().is_none_or(|p| path.starts_with(p)) {
            let len = r.path_prefix.as_deref().map_or(0, str::len);
            if best.is_none_or(|b| len > b.path_prefix.as_deref().map_or(0, str::len)) {
                best = Some(r);
            }
        }
    }
    best
}

/// A per-pod CA (minted once at boot). The private key stays in memory here and
/// is NEVER written to disk — only the public cert PEM is exported. Leaf certs
/// for injection hosts are generated on demand and cached by SNI.
pub struct PerPodCa {
    issuer: Issuer<'static, KeyPair>,
    ca_pem: String,
    #[cfg_attr(not(test), allow(dead_code))]
    ca_der: CertificateDer<'static>,
    leaf_cache: Mutex<HashMap<String, Arc<CertifiedKey>>>,
}

impl PerPodCa {
    /// Mints a fresh CA. Call once per process boot.
    pub fn generate() -> anyhow::Result<Self> {
        let ca_key = KeyPair::generate()?;
        let mut params = CertificateParams::new(Vec::new())?;
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        params.distinguished_name.push(DnType::CommonName, "cctui guard-proxy per-pod CA");
        let ca_cert = params.self_signed(&ca_key)?;
        let ca_pem = ca_cert.pem();
        let ca_der = ca_cert.der().clone();
        let issuer = Issuer::new(params, ca_key);
        Ok(Self { issuer, ca_pem, ca_der, leaf_cache: Mutex::new(HashMap::new()) })
    }

    /// The public CA cert in PEM (safe to write to the shared emptyDir).
    #[must_use]
    pub fn ca_pem(&self) -> &str {
        &self.ca_pem
    }

    /// The public CA cert in DER (used to trust the injector in tests).
    #[cfg(test)]
    #[must_use]
    pub const fn ca_der(&self) -> &CertificateDer<'static> {
        &self.ca_der
    }

    /// Returns (minting + caching) a leaf `CertifiedKey` for `host`, signed by
    /// this CA. Repeat calls for the same host return the cached `Arc`.
    pub fn leaf_for(&self, host: &str) -> anyhow::Result<Arc<CertifiedKey>> {
        {
            let cache = self.leaf_cache.lock().expect("leaf cache poisoned");
            if let Some(ck) = cache.get(host) {
                return Ok(ck.clone());
            }
        }
        let leaf_key = KeyPair::generate()?;
        let mut params = CertificateParams::new(vec![host.to_owned()])?;
        params.distinguished_name.push(DnType::CommonName, host);
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        params.use_authority_key_identifier_extension = true;
        let leaf = params.signed_by(&leaf_key, &self.issuer)?;

        let key_der = PrivateKeyDer::Pkcs8(leaf_key.serialize_der().into());
        let signing_key = any_supported_type(&key_der)
            .map_err(|e| anyhow::anyhow!("unsupported leaf key: {e}"))?;
        let certified = Arc::new(CertifiedKey::new(vec![leaf.der().clone()], signing_key));

        let mut cache = self.leaf_cache.lock().expect("leaf cache poisoned");
        Ok(cache.entry(host.to_owned()).or_insert(certified).clone())
    }
}

impl std::fmt::Debug for PerPodCa {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PerPodCa").finish_non_exhaustive()
    }
}

/// rustls cert resolver minting per-SNI leaf certs from the per-pod CA.
#[derive(Debug)]
struct SniResolver(Arc<PerPodCa>);

impl ResolvesServerCert for SniResolver {
    fn resolve(&self, client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let host = client_hello.server_name()?.to_owned();
        match self.0.leaf_for(&host) {
            Ok(ck) => Some(ck),
            Err(e) => {
                tracing::warn!("leaf mint for {host} failed: {e}");
                None
            }
        }
    }
}

/// Ensures a process-default rustls crypto provider is installed. Idempotent.
fn install_crypto() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

/// TLS-terminating credential injector for the configured host set.
pub struct Injector {
    acceptor: TlsAcceptor,
    connector: TlsConnector,
    secrets: Arc<SecretSource>,
    policy: InjectionPolicy,
    /// When present, the `github` service is served a freshly-minted GitHub App
    /// installation token (CCT-722) instead of the stored `github` credential.
    ghapp: Option<Arc<GhAppMinter>>,
}

impl Injector {
    #[allow(clippy::unnecessary_wraps)]
    pub fn new(
        ca: Arc<PerPodCa>,
        secrets: Arc<SecretSource>,
        policy: InjectionPolicy,
        ghapp: Option<Arc<GhAppMinter>>,
    ) -> anyhow::Result<Self> {
        install_crypto();
        let mut server_config = ServerConfig::builder()
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(SniResolver(ca)));
        server_config.alpn_protocols = vec![b"http/1.1".to_vec()];
        let acceptor = TlsAcceptor::from(Arc::new(server_config));
        let connector = Self::build_connector(Vec::new());
        Ok(Self { acceptor, connector, secrets, policy, ghapp })
    }

    fn build_connector(extra_roots: Vec<CertificateDer<'static>>) -> TlsConnector {
        install_crypto();
        let mut roots = RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        for cert in extra_roots {
            let _ = roots.add(cert);
        }
        let mut config =
            ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        TlsConnector::from(Arc::new(config))
    }

    /// True if `host` (no port) is on the injection allow-list.
    #[must_use]
    pub fn should_inject(&self, host: &str) -> bool {
        !self.policy.rules_for(host).is_empty()
    }

    /// Terminates TLS on `conn`, injects credentials per request, and forwards
    /// upstream. `prefix` is the bytes already peeked off `conn` (the
    /// `ClientHello`) that must be replayed into the TLS handshake.
    pub async fn handle(
        &self,
        conn: TcpStream,
        prefix: Vec<u8>,
        host: &str,
        port: u16,
    ) -> anyhow::Result<()> {
        let host = host.to_ascii_lowercase();
        let rules: Vec<InjectionRule> = self.policy.rules_for(&host).into_iter().cloned().collect();
        anyhow::ensure!(!rules.is_empty(), "no injection rule for {host}");

        let mut client = self.acceptor.accept(PrefixedStream::new(conn, prefix)).await?;
        let server_name = ServerName::try_from(host.clone())?;
        let upstream = tokio::time::timeout(
            Duration::from_secs(10),
            TcpStream::connect((host.as_str(), port)),
        )
        .await??;
        let mut upstream = self.connector.connect(server_name, upstream).await?;

        self.pump(&mut client, &mut upstream, &host, &rules).await
    }

    async fn pump<C, U>(
        &self,
        client: &mut C,
        upstream: &mut U,
        host: &str,
        rules: &[InjectionRule],
    ) -> anyhow::Result<()>
    where
        C: AsyncRead + AsyncWrite + Unpin,
        U: AsyncRead + AsyncWrite + Unpin,
    {
        let mut cbuf = HttpBuf::default();
        let mut ubuf = HttpBuf::default();
        loop {
            let Some(head) = cbuf.read_head(client).await? else {
                return Ok(()); // client closed
            };
            let method = request_method(&head);
            // Every rule path-scoped and none matching ⇒ forward the agent's
            // head unchanged (same fail-closed stance as a credential miss).
            let outbound = match select_rule(rules, &request_path(&head).unwrap_or_default()) {
                Some(rule) => self.inject_head(host, rule, &head).await,
                None => head.clone(),
            };
            upstream.write_all(&outbound).await?;
            cbuf.relay_body(client, upstream, body_framing(&head, false)).await?;
            upstream.flush().await?;

            let Some(resp_head) = ubuf.read_head(upstream).await? else {
                return Ok(()); // upstream closed after the request
            };
            client.write_all(&resp_head).await?;
            ubuf.relay_body(upstream, client, response_framing(&resp_head, method.as_deref()))
                .await?;
            client.flush().await?;

            if wants_close(&head) || wants_close(&resp_head) {
                return Ok(());
            }
        }
    }

    /// Builds the outbound request head. On a successful fetch: strip the
    /// agent's auth and substitute the real credential. On NotFound/Backend:
    /// forward the ORIGINAL head unchanged (fail-closed — never a blank secret).
    async fn inject_head(&self, host: &str, rule: &InjectionRule, head: &[u8]) -> Vec<u8> {
        match self.resolve_credential(rule).await {
            Ok(cred) => {
                let cookie = match (&rule.shape, &rule.cookie_secret) {
                    (AuthShape::BearerCookie { .. }, Some(r)) => {
                        self.secrets.fetch(r).await.ok().map(|c| c.expose().to_owned())
                    }
                    _ => None,
                };
                rewrite_head(head, &rule.shape, cred.expose(), cookie.as_deref())
            }
            Err(SecretError::NotFound { .. }) => {
                tracing::debug!(
                    "no credential for {}/{host}; forwarding agent header unchanged",
                    rule.service
                );
                head.to_vec()
            }
            Err(SecretError::Backend(e)) => {
                tracing::warn!(
                    "secret backend error for {}/{host} (forwarding agent header): {e}",
                    rule.service
                );
                head.to_vec()
            }
        }
    }

    /// Resolves the credential for a rule. A configured GitHub-App provider
    /// mints a short-lived installation token for its service label; a
    /// `NotFound` there (no App key provisioned) falls back to the rule's own
    /// secret ref, which is what keeps the App path inert until an operator
    /// wires it up.
    async fn resolve_credential(&self, rule: &InjectionRule) -> Result<Credential, SecretError> {
        if let Some(app) = &self.ghapp
            && app.handles(&rule.service)
        {
            match app.mint().await {
                Ok(cred) => return Ok(cred),
                Err(SecretError::NotFound { .. }) => {}
                Err(e) => return Err(e),
            }
        }
        self.secrets.fetch(&rule.secret).await
    }
}

/// Rebuilds an HTTP/1.1 request head, removing every agent-supplied auth carrier
/// and substituting the injected one. Operates on the head bytes only; the body
/// is streamed untouched. Only ever called with a real `credential`.
fn rewrite_head(head: &[u8], shape: &AuthShape, credential: &str, cookie: Option<&str>) -> Vec<u8> {
    let text = String::from_utf8_lossy(head);
    let mut lines = text.split("\r\n");
    let request_line = lines.next().unwrap_or_default();

    let mut kept: Vec<String> = Vec::new();
    for line in lines {
        if line.is_empty() {
            break; // end of header block
        }
        let name = line.split(':').next().unwrap_or_default().trim();
        if name.eq_ignore_ascii_case("authorization") {
            continue; // always strip the agent's Authorization
        }
        if let AuthShape::BearerCookie { cookie_name } = shape
            && name.eq_ignore_ascii_case("cookie")
        {
            if let Some(rewritten) = strip_named_cookie(line, cookie_name)
                && !rewritten.is_empty()
            {
                kept.push(rewritten);
            }
            continue;
        }
        kept.push(line.to_owned());
    }

    match shape {
        AuthShape::Bearer | AuthShape::BearerCookie { .. } => {
            kept.push(format!("Authorization: Bearer {credential}"));
        }
        AuthShape::Basic { username } => {
            let encoded = base64::engine::general_purpose::STANDARD
                .encode(format!("{username}:{credential}"));
            kept.push(format!("Authorization: Basic {encoded}"));
        }
    }
    if let AuthShape::BearerCookie { cookie_name } = shape
        && let Some(c) = cookie
    {
        kept.push(format!("Cookie: {cookie_name}={c}"));
    }

    let mut out = String::with_capacity(head.len() + 128);
    out.push_str(request_line);
    out.push_str("\r\n");
    for line in kept {
        out.push_str(&line);
        out.push_str("\r\n");
    }
    out.push_str("\r\n");
    out.into_bytes()
}

/// Removes the `<name>=<value>` pair from a `Cookie:` header line, returning
/// the line without it, or `Some("")` if nothing but that cookie remained.
fn strip_named_cookie(line: &str, name: &str) -> Option<String> {
    let value = line.split_once(':')?.1.trim();
    let prefix = format!("{name}=");
    let kept: Vec<&str> = value
        .split(';')
        .map(str::trim)
        .filter(|pair| !pair.is_empty() && !pair.starts_with(&prefix))
        .collect();
    if kept.is_empty() { Some(String::new()) } else { Some(format!("Cookie: {}", kept.join("; "))) }
}

// ---- HTTP/1.1 message framing ----------------------------------------------

/// Body framing of an HTTP/1.1 message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Framing {
    None,
    Length(usize),
    Chunked,
    UntilClose,
}

const MAX_HEAD: usize = 64 * 1024;

/// A tiny buffered HTTP reader: reads a message head (up to `\r\n\r\n`) and
/// keeps any bytes read past it for the body relay.
#[derive(Default)]
struct HttpBuf {
    buf: Vec<u8>,
}

impl HttpBuf {
    /// Reads bytes until the end of the header block. Returns the head bytes
    /// (including the terminating `\r\n\r\n`), or `None` on a clean EOF before
    /// any head. Leftover bytes stay in `self.buf` for the body relay.
    async fn read_head<S: AsyncRead + Unpin>(
        &mut self,
        stream: &mut S,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        let mut scan_from = 0usize;
        loop {
            if let Some(pos) = find_head_end(&self.buf, scan_from) {
                return Ok(Some(self.buf.drain(..pos).collect()));
            }
            scan_from = self.buf.len().saturating_sub(3);
            if self.buf.len() > MAX_HEAD {
                anyhow::bail!("HTTP head exceeds {MAX_HEAD} bytes");
            }
            let mut chunk = [0u8; 8192];
            let n = stream.read(&mut chunk).await?;
            if n == 0 {
                if self.buf.is_empty() {
                    return Ok(None);
                }
                anyhow::bail!("connection closed mid-head");
            }
            self.buf.extend_from_slice(&chunk[..n]);
        }
    }

    /// Relays a message body from `src` to `dst` per `framing`, draining any
    /// buffered leftover first.
    async fn relay_body<S, D>(
        &mut self,
        src: &mut S,
        dst: &mut D,
        framing: Framing,
    ) -> anyhow::Result<()>
    where
        S: AsyncRead + Unpin,
        D: AsyncWrite + Unpin,
    {
        match framing {
            Framing::None => Ok(()),
            Framing::Length(remaining) => self.relay_counted(src, dst, remaining).await,
            Framing::Chunked => self.relay_chunked(src, dst).await,
            Framing::UntilClose => {
                if !self.buf.is_empty() {
                    dst.write_all(&self.buf).await?;
                    self.buf.clear();
                }
                let mut chunk = [0u8; 8192];
                loop {
                    let n = src.read(&mut chunk).await?;
                    if n == 0 {
                        return Ok(());
                    }
                    dst.write_all(&chunk[..n]).await?;
                }
            }
        }
    }

    async fn relay_counted<S, D>(
        &mut self,
        src: &mut S,
        dst: &mut D,
        mut remaining: usize,
    ) -> anyhow::Result<()>
    where
        S: AsyncRead + Unpin,
        D: AsyncWrite + Unpin,
    {
        if !self.buf.is_empty() {
            let take = self.buf.len().min(remaining);
            dst.write_all(&self.buf[..take]).await?;
            self.buf.drain(..take);
            remaining -= take;
        }
        let mut chunk = [0u8; 8192];
        while remaining > 0 {
            // Never read past `remaining`: the excess belongs to the next
            // message/chunk head and would be dropped.
            let cap = remaining.min(chunk.len());
            let n = src.read(&mut chunk[..cap]).await?;
            if n == 0 {
                anyhow::bail!("closed with {remaining} body bytes outstanding");
            }
            dst.write_all(&chunk[..n]).await?;
            remaining -= n;
        }
        Ok(())
    }

    /// Relays a chunked body verbatim, stopping after the terminating
    /// zero-length chunk and its trailers.
    async fn relay_chunked<S, D>(&mut self, src: &mut S, dst: &mut D) -> anyhow::Result<()>
    where
        S: AsyncRead + Unpin,
        D: AsyncWrite + Unpin,
    {
        loop {
            let line = self.read_line(src).await?;
            dst.write_all(&line).await?;
            let size_str =
                std::str::from_utf8(&line).unwrap_or("").split(';').next().unwrap_or("").trim();
            let size = usize::from_str_radix(size_str, 16)
                .map_err(|_| anyhow::anyhow!("bad chunk size {size_str:?}"))?;
            if size == 0 {
                loop {
                    let trailer = self.read_line(src).await?;
                    dst.write_all(&trailer).await?;
                    if trailer == b"\r\n" || trailer == b"\n" {
                        return Ok(());
                    }
                }
            }
            self.relay_counted(src, dst, size + 2).await?; // chunk data + CRLF
        }
    }

    /// Reads a single LF-terminated line (including the terminator).
    async fn read_line<S: AsyncRead + Unpin>(&mut self, src: &mut S) -> anyhow::Result<Vec<u8>> {
        loop {
            if let Some(idx) = self.buf.iter().position(|&b| b == b'\n') {
                return Ok(self.buf.drain(..=idx).collect());
            }
            let mut chunk = [0u8; 512];
            let n = src.read(&mut chunk).await?;
            if n == 0 {
                anyhow::bail!("closed mid-line");
            }
            self.buf.extend_from_slice(&chunk[..n]);
        }
    }
}

fn find_head_end(buf: &[u8], from: usize) -> Option<usize> {
    if buf.len() < 4 {
        return None;
    }
    let start = from.min(buf.len());
    buf[start..].windows(4).position(|w| w == b"\r\n\r\n").map(|p| start + p + 4)
}

/// The request method (uppercased) from a request head.
fn request_method(head: &[u8]) -> Option<String> {
    let line = head.split(|&b| b == b'\n').next()?;
    let method = line.split(|&b| b == b' ').next()?;
    Some(String::from_utf8_lossy(method).trim().to_ascii_uppercase())
}

/// The request path (query string stripped) from a request head.
fn request_path(head: &[u8]) -> Option<String> {
    let line = head.split(|&b| b == b'\n').next()?;
    let target = line.split(|&b| b == b' ').nth(1)?;
    let target = String::from_utf8_lossy(target);
    Some(target.split('?').next().unwrap_or_default().to_owned())
}

/// Case-insensitive header value lookup within a message head.
fn header_value<'a>(head: &'a [u8], name: &str) -> Option<&'a [u8]> {
    for line in head.split(|&b| b == b'\n').skip(1) {
        let line = trim_ascii(line);
        if let Some(colon) = line.iter().position(|&b| b == b':')
            && line[..colon].eq_ignore_ascii_case(name.as_bytes())
        {
            return Some(trim_ascii(&line[colon + 1..]));
        }
    }
    None
}

const fn trim_ascii(mut b: &[u8]) -> &[u8] {
    while let [first, rest @ ..] = b {
        if first.is_ascii_whitespace() {
            b = rest;
        } else {
            break;
        }
    }
    while let [rest @ .., last] = b {
        if last.is_ascii_whitespace() {
            b = rest;
        } else {
            break;
        }
    }
    b
}

fn contains_token(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w.eq_ignore_ascii_case(needle))
}

fn body_framing(head: &[u8], is_response: bool) -> Framing {
    if header_value(head, "transfer-encoding").is_some_and(|v| contains_token(v, b"chunked")) {
        return Framing::Chunked;
    }
    if let Some(len) = header_value(head, "content-length")
        .and_then(|v| std::str::from_utf8(v).ok())
        .and_then(|v| v.trim().parse::<usize>().ok())
    {
        return Framing::Length(len);
    }
    if is_response { Framing::UntilClose } else { Framing::None }
}

/// Response framing per RFC 7230: HEAD, 1xx, 204, 304 carry no body regardless
/// of headers.
fn response_framing(head: &[u8], request_method: Option<&str>) -> Framing {
    let status = response_status(head);
    if request_method == Some("HEAD")
        || matches!(status, Some(204 | 304))
        || status.is_some_and(|s| (100..200).contains(&s))
    {
        return Framing::None;
    }
    body_framing(head, true)
}

fn response_status(head: &[u8]) -> Option<u16> {
    let line = head.split(|&b| b == b'\n').next()?;
    let mut parts = line.split(|&b| b == b' ');
    parts.next()?; // HTTP/1.1
    std::str::from_utf8(parts.next()?).ok()?.trim().parse().ok()
}

fn wants_close(head: &[u8]) -> bool {
    if let Some(conn) = header_value(head, "connection") {
        if contains_token(conn, b"close") {
            return true;
        }
        if contains_token(conn, b"keep-alive") {
            return false;
        }
    }
    // No Connection header: HTTP/1.0 defaults to close, HTTP/1.1 to keep-alive.
    head.split(|&b| b == b'\n').next().is_some_and(|line| contains_token(line, b"HTTP/1.0"))
}

/// A `TcpStream` with a leading byte prefix replayed before the socket's own
/// reads — feeds the already-peeked `ClientHello` into the TLS handshake.
struct PrefixedStream {
    inner: TcpStream,
    prefix: Vec<u8>,
    pos: usize,
}

impl PrefixedStream {
    const fn new(inner: TcpStream, prefix: Vec<u8>) -> Self {
        Self { inner, prefix, pos: 0 }
    }
}

impl AsyncRead for PrefixedStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.pos < self.prefix.len() {
            let remaining = &self.prefix[self.pos..];
            let n = remaining.len().min(buf.remaining());
            buf.put_slice(&remaining[..n]);
            self.pos += n;
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for PrefixedStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::large_futures)]
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn head(body: &str) -> Vec<u8> {
        body.replace('\n', "\r\n").into_bytes()
    }

    fn as_text(v: &[u8]) -> String {
        String::from_utf8(v.to_vec()).unwrap()
    }

    fn test_ref(var: &str) -> SecretRef {
        SecretRef::Env { var: var.to_owned() }
    }

    fn rule_from(spec: HostSpec) -> InjectionRule {
        let cookie_secret =
            matches!(spec.shape, AuthShape::BearerCookie { .. }).then(|| test_ref("TEST_COOKIE"));
        InjectionRule {
            host: spec.host,
            path_prefix: None,
            service: spec.service,
            shape: spec.shape,
            secret: test_ref("TEST_SECRET"),
            cookie_secret,
        }
    }

    #[test]
    fn parse_inject_host_forms() {
        assert_eq!(parse_inject_host("api.github.com").unwrap().service, "github");
        assert!(matches!(parse_inject_host("api.github.com").unwrap().shape, AuthShape::Bearer));
        assert!(matches!(parse_inject_host("github.com").unwrap().shape, AuthShape::Basic { .. }));
        let npm = parse_inject_host("Registry.NPMJS.org").unwrap();
        assert_eq!(npm.host, "registry.npmjs.org");
        assert_eq!(npm.service, "npm");
        assert!(matches!(
            parse_inject_host("slack.com").unwrap().shape,
            AuthShape::BearerCookie { .. }
        ));

        let unknown = parse_inject_host("example.com").unwrap();
        assert_eq!(unknown.service, "example.com");
        assert!(matches!(unknown.shape, AuthShape::Bearer));

        let ghe = parse_inject_host("git.internal=ghe:git").unwrap();
        assert_eq!(ghe.service, "ghe");
        assert!(matches!(ghe.shape, AuthShape::Basic { .. }));
        let yt = parse_inject_host("yt.example=youtrack").unwrap();
        assert_eq!(yt.service, "youtrack");
        assert!(matches!(yt.shape, AuthShape::Bearer));
        assert!(matches!(
            parse_inject_host("chat.example=chat:cookie").unwrap().shape,
            AuthShape::BearerCookie { .. }
        ));

        assert!(parse_inject_host("").is_none());
        assert!(parse_inject_host("   ").is_none());
    }

    #[test]
    fn policy_matches_case_insensitively() {
        let policy = InjectionPolicy::new(vec![
            rule_from(parse_inject_host("api.github.com").unwrap()),
            rule_from(parse_inject_host("github.com").unwrap()),
        ]);
        assert_eq!(policy.rules_for("API.GitHub.com")[0].service, "github");
        assert!(matches!(policy.rules_for("github.com")[0].shape, AuthShape::Basic { .. }));
        assert!(policy.rules_for("example.com").is_empty());
    }

    #[test]
    fn inject_config_parses_shapes_refs_and_templating() {
        let json = r#"[
            {"host": "api.github.com", "service": "github",
             "secret": "vault:kvmount/data/cctui/workers#GITHUB_TOKEN_${IDENTITY}"},
            {"host": "GitHub.com", "service": "github", "shape": "git",
             "secret": "vault:kvmount/data/cctui/workers#GITHUB_TOKEN_${IDENTITY}"},
            {"host": "chat.example.com", "shape": "bearer+cookie", "cookie_name": "sess",
             "secret": "env:CHAT_TOKEN", "cookie_secret": "k8s:chat-creds#cookie"},
            {"host": "internal.example.com", "shape": "basic", "username": "svc",
             "secret": "aws-sm:prod/internal#password"}
        ]"#;
        let rules = load_inject_config(json, "acme").unwrap();
        assert_eq!(rules.len(), 4);

        assert_eq!(rules[0].host, "api.github.com");
        assert!(matches!(rules[0].shape, AuthShape::Bearer));
        assert_eq!(
            rules[0].secret,
            SecretRef::parse("vault:kvmount/data/cctui/workers#GITHUB_TOKEN_ACME").unwrap()
        );

        assert_eq!(rules[1].host, "github.com");
        assert!(
            matches!(&rules[1].shape, AuthShape::Basic { username } if username == "x-access-token")
        );

        assert!(
            matches!(&rules[2].shape, AuthShape::BearerCookie { cookie_name } if cookie_name == "sess")
        );
        assert_eq!(rules[2].service, "chat.example.com");
        assert_eq!(
            rules[2].cookie_secret,
            Some(SecretRef::parse("k8s:chat-creds#cookie").unwrap())
        );

        assert!(matches!(&rules[3].shape, AuthShape::Basic { username } if username == "svc"));
    }

    #[test]
    fn inject_config_parses_path_prefix_and_selects_by_longest_match() {
        let json = r#"[
            {"host": "github.com", "shape": "git", "path_prefix": "/Acme/context-packs",
             "secret": "env:PACK_TOKEN"},
            {"host": "github.com", "shape": "git",
             "secret": "env:GITHUB_TOKEN_${IDENTITY}"}
        ]"#;
        let rules = load_inject_config(json, "zephyr").unwrap();
        assert_eq!(rules[0].path_prefix.as_deref(), Some("/Acme/context-packs"));
        assert_eq!(rules[1].path_prefix, None);

        let pack = select_rule(&rules, "/Acme/context-packs/info/refs").unwrap();
        assert_eq!(pack.secret, SecretRef::parse("env:PACK_TOKEN").unwrap());
        let other = select_rule(&rules, "/Acme/work-repo/info/refs").unwrap();
        assert_eq!(other.secret, SecretRef::parse("env:GITHUB_TOKEN_ZEPHYR").unwrap());
        assert!(select_rule(&rules[..1], "/Other/repo").is_none());
    }

    #[test]
    fn inject_config_rejects_relative_path_prefix() {
        let json = r#"[{"host": "a.com", "path_prefix": "no-slash", "secret": "env:X"}]"#;
        assert!(load_inject_config(json, "acme").is_err());
    }

    #[test]
    fn request_path_strips_query() {
        assert_eq!(
            request_path(b"GET /a/b/info/refs?service=git-upload-pack HTTP/1.1\r\n\r\n").as_deref(),
            Some("/a/b/info/refs")
        );
        assert_eq!(request_path(b"POST /x HTTP/1.1\r\n\r\n").as_deref(), Some("/x"));
    }

    #[test]
    fn inject_config_rejects_bad_entries() {
        for (json, why) in [
            (r#"[{"host": "a.com", "secret": "bogus-ref"}]"#, "unknown ref scheme"),
            (r#"[{"host": "a.com", "shape": "sigv4", "secret": "env:X"}]"#, "unknown shape"),
            (
                r#"[{"host": "a.com", "shape": "cookie", "secret": "env:X"}]"#,
                "missing cookie_secret",
            ),
            (r#"[{"host": "", "secret": "env:X"}]"#, "empty host"),
            (
                r#"[{"host": "a.com", "secret": "env:CRED_${IDENTITY}"}]"#,
                "placeholder w/o identity",
            ),
            (r#"[{"host": "a.com", "secret": "env:X", "typo_field": 1}]"#, "unknown field"),
        ] {
            let identity = if why.contains("identity") { "" } else { "acme" };
            assert!(load_inject_config(json, identity).is_err(), "must reject: {why}");
        }
    }

    #[test]
    fn bearer_strips_agent_auth_and_substitutes() {
        let h = head(
            "GET /user HTTP/1.1\nHost: api.github.com\nAuthorization: Bearer GARBAGE\nAccept: */*\n\n",
        );
        let out = as_text(&rewrite_head(&h, &AuthShape::Bearer, "real-token", None));
        assert!(out.starts_with("GET /user HTTP/1.1\r\n"));
        assert!(out.contains("Host: api.github.com\r\n"));
        assert!(out.contains("Accept: */*\r\n"));
        assert!(out.contains("Authorization: Bearer real-token\r\n"));
        assert!(!out.contains("GARBAGE"), "agent auth must be gone: {out}");
        assert!(out.ends_with("\r\n\r\n"));
    }

    #[test]
    fn basic_rewrites_git_credentials() {
        let agent = base64::engine::general_purpose::STANDARD.encode("user:agentpat");
        let h = head(&format!(
            "POST /acme/repo.git/git-receive-pack HTTP/1.1\nHost: github.com\nAuthorization: Basic {agent}\n\n"
        ));
        let out = as_text(&rewrite_head(
            &h,
            &AuthShape::Basic { username: "x-access-token".to_owned() },
            "ghp_real",
            None,
        ));
        let expected = base64::engine::general_purpose::STANDARD.encode("x-access-token:ghp_real");
        assert!(out.contains(&format!("Authorization: Basic {expected}\r\n")), "{out}");
        assert!(!out.contains(&agent), "agent basic creds must be gone");
    }

    #[test]
    fn bearer_cookie_injects_bearer_and_cookie_stripping_agent_cookie() {
        let h = head(
            "POST /api/conversations.list HTTP/1.1\nHost: slack.com\nCookie: b=1; d=agentcookie; lc=2\nAuthorization: Bearer xoxc-agent\n\n",
        );
        let shape = AuthShape::BearerCookie { cookie_name: "d".to_owned() };
        let out = as_text(&rewrite_head(&h, &shape, "xoxc-real", Some("xoxd-real")));
        assert!(out.contains("Authorization: Bearer xoxc-real\r\n"), "{out}");
        assert!(out.contains("Cookie: b=1; lc=2\r\n"), "non-d cookies kept: {out}");
        assert!(out.contains("Cookie: d=xoxd-real\r\n"), "real d cookie injected: {out}");
        assert!(!out.contains("agentcookie"));
        assert!(!out.contains("xoxc-agent"));
    }

    #[tokio::test]
    async fn relay_counted_never_consumes_past_the_body() {
        let (mut w, mut src) = tokio::io::duplex(4);
        tokio::spawn(async move {
            w.write_all(b"HELLOWORLD").await.unwrap();
        });
        let mut buf = HttpBuf::default();
        let mut dst = Vec::new();
        buf.relay_counted(&mut src, &mut dst, 5).await.unwrap();
        assert_eq!(dst, b"HELLO");

        let mut rest = vec![0u8; 5];
        src.read_exact(&mut rest).await.unwrap();
        assert_eq!(&rest, b"WORLD", "bytes after the body must stay unconsumed");
    }

    #[tokio::test]
    async fn relay_chunked_survives_coalesced_segments() {
        let (mut w, mut src) = tokio::io::duplex(7);
        tokio::spawn(async move {
            w.write_all(b"5\r\nHELLO\r\nA\r\nWORLDWORLD\r\n0\r\n\r\n").await.unwrap();
        });
        let mut buf = HttpBuf::default();
        let mut dst = Vec::new();
        buf.relay_chunked(&mut src, &mut dst).await.unwrap();
        assert_eq!(dst, b"5\r\nHELLO\r\nA\r\nWORLDWORLD\r\n0\r\n\r\n");
    }

    #[test]
    fn framing_detection() {
        assert_eq!(
            body_framing(b"POST / HTTP/1.1\r\nContent-Length: 5\r\n\r\n", false),
            Framing::Length(5)
        );
        assert_eq!(
            body_framing(b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n", false),
            Framing::Chunked
        );
        assert_eq!(body_framing(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n", false), Framing::None);
        assert_eq!(body_framing(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n", true), Framing::UntilClose);
        assert_eq!(
            response_framing(b"HTTP/1.1 204 No Content\r\n\r\n", Some("GET")),
            Framing::None
        );
        let ok = b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n";
        assert_eq!(response_framing(ok, Some("HEAD")), Framing::None);
        assert_eq!(response_framing(ok, Some("GET")), Framing::Length(10));
    }

    #[test]
    fn wants_close_detection() {
        assert!(wants_close(b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n"));
        assert!(!wants_close(b"GET / HTTP/1.1\r\nConnection: keep-alive\r\n\r\n"));
        assert!(!wants_close(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n"));
        assert!(wants_close(b"GET / HTTP/1.0\r\nHost: x\r\n\r\n"));
    }

    #[test]
    fn ca_generates_public_pem_and_key_never_leaks() {
        let ca = PerPodCa::generate().unwrap();
        assert!(ca.ca_pem().contains("BEGIN CERTIFICATE"));
        assert!(!ca.ca_pem().contains("PRIVATE KEY"), "CA PEM must not carry the key");
        assert!(!ca.ca_der().is_empty());
    }

    #[test]
    fn leaf_cert_generation_is_cached_and_distinct_per_host() {
        let ca = PerPodCa::generate().unwrap();
        let a = ca.leaf_for("api.github.com").unwrap();
        let b = ca.leaf_for("api.github.com").unwrap();
        assert!(Arc::ptr_eq(&a, &b), "same host must hit the cache");
        let c = ca.leaf_for("github.com").unwrap();
        assert!(!Arc::ptr_eq(&a, &c), "different host mints a distinct leaf");
    }

    // ---- End-to-end injection over real TLS ---------------------------------
    //
    // A successful handshake proves the minted leaf chains to the per-pod CA:
    // each rustls client trusts ONLY that CA, so a bad signature would fail the
    // handshake before any HTTP flows.

    struct Upstream {
        addr: std::net::SocketAddr,
        ca: Arc<PerPodCa>,
        requests: Arc<AtomicUsize>,
    }

    /// Minimal HTTPS upstream: rustls-terminated with a per-pod-CA leaf, echoes
    /// the received `Authorization` header value as the response body.
    async fn spawn_upstream() -> Upstream {
        install_crypto();
        let ca = Arc::new(PerPodCa::generate().unwrap());
        let mut cfg = ServerConfig::builder()
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(SniResolver(ca.clone())));
        cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
        let acceptor = TlsAcceptor::from(Arc::new(cfg));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let rc = requests.clone();
        tokio::spawn(async move {
            loop {
                let Ok((sock, _)) = listener.accept().await else { return };
                let acceptor = acceptor.clone();
                let rc = rc.clone();
                tokio::spawn(async move {
                    let Ok(mut tls) = acceptor.accept(sock).await else { return };
                    let mut buf = HttpBuf::default();
                    if let Ok(Some(reqhead)) = buf.read_head(&mut tls).await {
                        rc.fetch_add(1, Ordering::SeqCst);
                        let mut sink = tokio::io::sink();
                        let _ = buf
                            .relay_body(&mut tls, &mut sink, body_framing(&reqhead, false))
                            .await;
                        let auth = header_value(&reqhead, "authorization")
                            .map(|v| String::from_utf8_lossy(v).into_owned())
                            .unwrap_or_default();
                        let body = auth.into_bytes();
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = tls.write_all(resp.as_bytes()).await;
                        let _ = tls.write_all(&body).await;
                        let _ = tls.flush().await;
                    }
                });
            }
        });
        Upstream { addr, ca, requests }
    }

    struct MockBackend(Result<&'static str, ()>);

    #[async_trait::async_trait]
    impl crate::secrets::RefResolver for MockBackend {
        async fn resolve(&self, r: &SecretRef) -> Result<crate::secrets::Credential, SecretError> {
            match self.0 {
                Ok(v) => Ok(crate::secrets::Credential::new(v.to_owned())),
                Err(()) => Err(SecretError::NotFound { what: r.to_string() }),
            }
        }
    }

    struct EchoRefBackend;

    #[async_trait::async_trait]
    impl crate::secrets::RefResolver for EchoRefBackend {
        async fn resolve(&self, r: &SecretRef) -> Result<crate::secrets::Credential, SecretError> {
            Ok(crate::secrets::Credential::new(r.to_string()))
        }
    }

    fn injector_for(
        up: &Upstream,
        backend: MockBackend,
        shape: AuthShape,
    ) -> (Injector, Arc<PerPodCa>) {
        let client_ca = Arc::new(PerPodCa::generate().unwrap());
        let secrets = Arc::new(SecretSource::new(Box::new(backend), Duration::from_secs(120)));
        let policy = InjectionPolicy::new(vec![InjectionRule {
            host: "localhost".to_owned(),
            path_prefix: None,
            service: "svc".to_owned(),
            shape,
            secret: test_ref("TEST_SECRET"),
            cookie_secret: None,
        }]);
        let mut inj = Injector::new(client_ca.clone(), secrets, policy, None).unwrap();
        // Trust the upstream's self-signed CA so the injector validates it.
        inj.connector = Injector::build_connector(vec![up.ca.ca_der().clone()]);
        (inj, client_ca)
    }

    /// Drives one request through the injector and returns the response body
    /// (which the upstream sets to the Authorization header it received).
    async fn roundtrip(
        inj: Arc<Injector>,
        client_ca: &Arc<PerPodCa>,
        upstream_port: u16,
        agent_auth: &str,
    ) -> String {
        roundtrip_path(inj, client_ca, upstream_port, "/user", agent_auth).await
    }

    async fn roundtrip_path(
        inj: Arc<Injector>,
        client_ca: &Arc<PerPodCa>,
        upstream_port: u16,
        path: &str,
        agent_auth: &str,
    ) -> String {
        install_crypto();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (sock, _) = listener.accept().await.unwrap();
            inj.handle(sock, Vec::new(), "localhost", upstream_port).await.unwrap();
        });

        let mut roots = RootCertStore::empty();
        roots.add(client_ca.ca_der().clone()).unwrap();
        let cfg = ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();
        let connector = TlsConnector::from(Arc::new(cfg));
        let sock = TcpStream::connect(addr).await.unwrap();
        let name = ServerName::try_from("localhost").unwrap();
        let mut tls = connector.connect(name, sock).await.unwrap();
        let req = format!(
            "GET {path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: {agent_auth}\r\nConnection: close\r\n\r\n"
        );
        tls.write_all(req.as_bytes()).await.unwrap();

        let mut buf = HttpBuf::default();
        let resp = buf.read_head(&mut tls).await.unwrap().unwrap();
        let mut body = Vec::new();
        buf.relay_body(&mut tls, &mut body, response_framing(&resp, Some("GET"))).await.unwrap();
        String::from_utf8(body).unwrap()
    }

    #[tokio::test]
    async fn bearer_injection_strips_and_substitutes() {
        let up = spawn_upstream().await;
        let (inj, ca) = injector_for(&up, MockBackend(Ok("REAL-TOKEN")), AuthShape::Bearer);
        let got = roundtrip(Arc::new(inj), &ca, up.addr.port(), "Bearer AGENT-GARBAGE").await;
        assert_eq!(got, "Bearer REAL-TOKEN", "upstream must see the injected token");
    }

    #[tokio::test]
    async fn garbage_agent_auth_is_ignored_and_call_succeeds() {
        let up = spawn_upstream().await;
        let (inj, ca) = injector_for(&up, MockBackend(Ok("REAL-TOKEN")), AuthShape::Bearer);
        let got = roundtrip(Arc::new(inj), &ca, up.addr.port(), "!!not a real header!!").await;
        assert_eq!(got, "Bearer REAL-TOKEN");
        assert_eq!(up.requests.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn git_basic_injection_rewrites_to_x_access_token() {
        let up = spawn_upstream().await;
        let shape = AuthShape::Basic { username: "x-access-token".to_owned() };
        let (inj, ca) = injector_for(&up, MockBackend(Ok("ghs_token")), shape);
        let got = roundtrip(Arc::new(inj), &ca, up.addr.port(), "Basic AGENTCREDS").await;
        let expected = base64::engine::general_purpose::STANDARD.encode("x-access-token:ghs_token");
        assert_eq!(got, format!("Basic {expected}"));
    }

    #[tokio::test]
    async fn not_found_forwards_agent_header_unchanged() {
        let up = spawn_upstream().await;
        let (inj, ca) = injector_for(&up, MockBackend(Err(())), AuthShape::Bearer);
        let got = roundtrip(Arc::new(inj), &ca, up.addr.port(), "Bearer AGENT-ORIGINAL").await;
        assert_eq!(got, "Bearer AGENT-ORIGINAL", "NotFound must forward the original header");
    }

    #[tokio::test]
    async fn path_scoped_rule_injects_its_own_secret_over_tls() {
        let up = spawn_upstream().await;
        let client_ca = Arc::new(PerPodCa::generate().unwrap());
        let secrets =
            Arc::new(SecretSource::new(Box::new(EchoRefBackend), Duration::from_secs(120)));
        let mk = |path_prefix: Option<&str>, var: &str| InjectionRule {
            host: "localhost".to_owned(),
            path_prefix: path_prefix.map(str::to_owned),
            service: "svc".to_owned(),
            shape: AuthShape::Bearer,
            secret: test_ref(var),
            cookie_secret: None,
        };
        let policy =
            InjectionPolicy::new(vec![mk(Some("/pack"), "PACK_TOKEN"), mk(None, "GENERIC_TOKEN")]);
        let mut inj = Injector::new(client_ca.clone(), secrets, policy, None).unwrap();
        inj.connector = Injector::build_connector(vec![up.ca.ca_der().clone()]);
        let inj = Arc::new(inj);

        let got =
            roundtrip_path(inj.clone(), &client_ca, up.addr.port(), "/pack/info/refs", "Bearer X")
                .await;
        assert_eq!(got, "Bearer env:PACK_TOKEN");
        let got = roundtrip_path(inj, &client_ca, up.addr.port(), "/other/repo", "Bearer X").await;
        assert_eq!(got, "Bearer env:GENERIC_TOKEN");
    }

    #[tokio::test]
    async fn should_inject_only_for_configured_hosts() {
        let up = spawn_upstream().await;
        let (inj, _ca) = injector_for(&up, MockBackend(Ok("t")), AuthShape::Bearer);
        assert!(inj.should_inject("localhost"));
        assert!(inj.should_inject("LOCALHOST"));
        // A non-injection host is never terminated — transparent.rs splices it
        // through byte-for-byte (covered by the transparent splice tests).
        assert!(!inj.should_inject("example.com"));
    }
}
