//! Ambient AWS credentials for `sigv4` rules without a static key pair.
//!
//! First configured source wins: env, web identity, container endpoint (ECS,
//! EKS Pod Identity), then `IMDSv2`.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::Context as _;

const REFRESH_MARGIN: Duration = Duration::from_mins(5);
const SESSION_NAME: &str = "cctui-guard-proxy";

/// ECS task and EKS Pod Identity agent endpoints, allowed over plain HTTP.
const CONTAINER_HTTP_HOSTS: [IpAddr; 3] = [
    IpAddr::V4(Ipv4Addr::new(169, 254, 170, 2)),
    IpAddr::V4(Ipv4Addr::new(169, 254, 170, 23)),
    IpAddr::V6(Ipv6Addr::new(0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x23)),
];

#[derive(Clone, PartialEq, Eq)]
pub struct AwsCredentials {
    pub key_id: String,
    pub secret_key: String,
    pub session_token: Option<String>,
    pub expiry: Option<SystemTime>,
}

impl AwsCredentials {
    /// Long-term credentials: no session token, no expiry.
    #[must_use]
    pub fn new(key_id: impl Into<String>, secret_key: impl Into<String>) -> Self {
        Self {
            key_id: key_id.into(),
            secret_key: secret_key.into(),
            session_token: None,
            expiry: None,
        }
    }

    fn fresh_at(&self, now: SystemTime) -> bool {
        self.expiry.is_none_or(|e| now + REFRESH_MARGIN < e)
    }

    fn valid_at(&self, now: SystemTime) -> bool {
        self.expiry.is_none_or(|e| now < e)
    }
}

impl fmt::Debug for AwsCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AwsCredentials")
            .field("key_id", &self.key_id)
            .field("expiry", &self.expiry)
            .finish_non_exhaustive()
    }
}

#[async_trait::async_trait]
pub trait CredentialSource: Send + Sync {
    async fn fetch(&self) -> anyhow::Result<AwsCredentials>;
}

pub type Clock = Arc<dyn Fn() -> SystemTime + Send + Sync>;
type EnvLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// A [`CredentialSource`] behind an expiry-aware cache. A failed refresh keeps
/// serving the cached credentials until they actually expire.
pub struct AmbientAws {
    source: Box<dyn CredentialSource>,
    now: Clock,
    cached: tokio::sync::Mutex<Option<AwsCredentials>>,
}

impl AmbientAws {
    pub fn default_chain() -> anyhow::Result<Self> {
        Ok(Self::new(Box::new(DefaultChain::from_process_env()?), Arc::new(SystemTime::now)))
    }

    #[must_use]
    pub fn new(source: Box<dyn CredentialSource>, now: Clock) -> Self {
        Self { source, now, cached: tokio::sync::Mutex::new(None) }
    }

    pub async fn credentials(&self) -> anyhow::Result<AwsCredentials> {
        let mut cached = self.cached.lock().await;
        let now = (self.now)();
        if let Some(c) = cached.as_ref()
            && c.fresh_at(now)
        {
            return Ok(c.clone());
        }
        match self.source.fetch().await {
            Ok(fresh) => {
                *cached = Some(fresh.clone());
                Ok(fresh)
            }
            Err(e) => match cached.as_ref() {
                Some(c) if c.valid_at(now) => {
                    tracing::warn!("ambient AWS credential refresh failed; using cached: {e:#}");
                    Ok(c.clone())
                }
                _ => Err(e),
            },
        }
    }
}

/// The default credential chain, reading the standard AWS environment variables.
pub struct DefaultChain {
    http: reqwest::Client,
    env: EnvLookup,
}

impl DefaultChain {
    pub fn from_process_env() -> anyhow::Result<Self> {
        Self::with_env(Arc::new(|k: &str| std::env::var(k).ok()))
    }

    fn with_env(env: EnvLookup) -> anyhow::Result<Self> {
        crate::inject::install_crypto();
        let http = reqwest::Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(10))
            .user_agent(SESSION_NAME)
            .build()?;
        Ok(Self { http, env })
    }

    fn var(&self, name: &str) -> Option<String> {
        (self.env)(name).map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
    }

    fn env_keys(&self) -> Option<AwsCredentials> {
        Some(AwsCredentials {
            key_id: self.var("AWS_ACCESS_KEY_ID")?,
            secret_key: self.var("AWS_SECRET_ACCESS_KEY")?,
            session_token: self.var("AWS_SESSION_TOKEN"),
            expiry: None,
        })
    }

    async fn web_identity(
        &self,
        token_file: &str,
        role_arn: &str,
    ) -> anyhow::Result<AwsCredentials> {
        let token = tokio::fs::read_to_string(token_file)
            .await
            .with_context(|| format!("reading web identity token {token_file}"))?;
        let session = self.var("AWS_ROLE_SESSION_NAME").unwrap_or_else(|| SESSION_NAME.to_owned());
        let resp = self
            .http
            .post(self.sts_endpoint())
            .form(&[
                ("Action", "AssumeRoleWithWebIdentity"),
                ("Version", "2011-06-15"),
                ("RoleArn", role_arn),
                ("RoleSessionName", session.as_str()),
                ("WebIdentityToken", token.trim()),
            ])
            .send()
            .await
            .context("sts AssumeRoleWithWebIdentity")?;
        let status = resp.status();
        let body = resp.text().await?;
        anyhow::ensure!(
            status.is_success(),
            "sts AssumeRoleWithWebIdentity: {status}: {}",
            xml_tag(&body, "Message").unwrap_or_default()
        );
        parse_sts_credentials(&body)
    }

    fn sts_endpoint(&self) -> String {
        if let Some(url) = self.var("AWS_ENDPOINT_URL_STS") {
            return url;
        }
        self.var("AWS_REGION").or_else(|| self.var("AWS_DEFAULT_REGION")).map_or_else(
            || "https://sts.amazonaws.com/".to_owned(),
            |region| format!("https://sts.{region}.amazonaws.com/"),
        )
    }

    fn container_uri(&self) -> anyhow::Result<Option<reqwest::Url>> {
        if let Some(relative) = self.var("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI") {
            return Ok(Some(reqwest::Url::parse(&format!("http://169.254.170.2{relative}"))?));
        }
        let Some(full) = self.var("AWS_CONTAINER_CREDENTIALS_FULL_URI") else { return Ok(None) };
        let url = reqwest::Url::parse(&full)
            .with_context(|| format!("AWS_CONTAINER_CREDENTIALS_FULL_URI {full:?}"))?;
        anyhow::ensure!(
            url.scheme() == "https" || (url.scheme() == "http" && container_http_host(&url)),
            "AWS_CONTAINER_CREDENTIALS_FULL_URI must be https, loopback, or the ECS / EKS \
             endpoint: {full}"
        );
        Ok(Some(url))
    }

    async fn container(&self, url: reqwest::Url) -> anyhow::Result<AwsCredentials> {
        let mut req = self.http.get(url);
        if let Some(token) = self.container_token().await? {
            req = req.header(reqwest::header::AUTHORIZATION, token);
        }
        let resp = req.send().await.context("container credentials endpoint")?;
        let status = resp.status();
        anyhow::ensure!(status.is_success(), "container credentials endpoint: {status}");
        parse_json_credentials(&resp.text().await?)
    }

    /// The token file is re-read on every fetch: the Pod Identity token rotates.
    async fn container_token(&self) -> anyhow::Result<Option<String>> {
        if let Some(path) = self.var("AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE") {
            let token = tokio::fs::read_to_string(&path)
                .await
                .with_context(|| format!("reading container authorization token {path}"))?;
            return Ok(Some(token.trim().to_owned()));
        }
        Ok(self.var("AWS_CONTAINER_AUTHORIZATION_TOKEN"))
    }

    async fn imds(&self) -> anyhow::Result<AwsCredentials> {
        let base = self
            .var("AWS_EC2_METADATA_SERVICE_ENDPOINT")
            .unwrap_or_else(|| "http://169.254.169.254".to_owned());
        let base = base.trim_end_matches('/');
        let token = self
            .http
            .put(format!("{base}/latest/api/token"))
            .header("x-aws-ec2-metadata-token-ttl-seconds", "21600")
            .send()
            .await
            .context("IMDS token")?
            .error_for_status()?
            .text()
            .await?;
        let roles_url = format!("{base}/latest/meta-data/iam/security-credentials/");
        let roles = self.imds_get(&roles_url, &token).await?;
        let role = roles
            .lines()
            .map(str::trim)
            .find(|r| !r.is_empty())
            .ok_or_else(|| anyhow::anyhow!("IMDS: no instance profile role"))?;
        parse_json_credentials(&self.imds_get(&format!("{roles_url}{role}"), &token).await?)
    }

    async fn imds_get(&self, url: &str, token: &str) -> anyhow::Result<String> {
        Ok(self
            .http
            .get(url)
            .header("x-aws-ec2-metadata-token", token)
            .send()
            .await
            .context("IMDS")?
            .error_for_status()?
            .text()
            .await?)
    }
}

#[async_trait::async_trait]
impl CredentialSource for DefaultChain {
    async fn fetch(&self) -> anyhow::Result<AwsCredentials> {
        if let Some(creds) = self.env_keys() {
            return Ok(creds);
        }
        if let (Some(token_file), Some(role_arn)) =
            (self.var("AWS_WEB_IDENTITY_TOKEN_FILE"), self.var("AWS_ROLE_ARN"))
        {
            return self.web_identity(&token_file, &role_arn).await;
        }
        if let Some(url) = self.container_uri()? {
            return self.container(url).await;
        }
        let imds_disabled =
            self.var("AWS_EC2_METADATA_DISABLED").is_some_and(|v| v.eq_ignore_ascii_case("true"));
        anyhow::ensure!(
            !imds_disabled,
            "no ambient AWS credentials: no env keys, web identity or container endpoint, and \
             IMDS is disabled"
        );
        self.imds().await
    }
}

fn container_http_host(url: &reqwest::Url) -> bool {
    let host = url.host_str().unwrap_or_default().trim_start_matches('[').trim_end_matches(']');
    host.parse::<IpAddr>().map_or_else(
        |_| host.eq_ignore_ascii_case("localhost"),
        |ip| ip.is_loopback() || CONTAINER_HTTP_HOSTS.contains(&ip),
    )
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
struct JsonCredentials {
    access_key_id: String,
    secret_access_key: String,
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    expiration: Option<String>,
    #[serde(default)]
    code: Option<String>,
}

/// The container endpoint and IMDS response body.
fn parse_json_credentials(body: &str) -> anyhow::Result<AwsCredentials> {
    let parsed: JsonCredentials = serde_json::from_str(body).map_err(|e| {
        anyhow::anyhow!(
            "credentials response is not the expected JSON (line {}, column {})",
            e.line(),
            e.column()
        )
    })?;
    if let Some(code) = parsed.code.as_deref() {
        anyhow::ensure!(code == "Success", "credentials response code {code:?}");
    }
    Ok(AwsCredentials {
        key_id: parsed.access_key_id,
        secret_key: parsed.secret_access_key,
        session_token: parsed.token.filter(|t| !t.is_empty()),
        expiry: parsed.expiration.as_deref().map(parse_expiry).transpose()?,
    })
}

fn parse_sts_credentials(xml: &str) -> anyhow::Result<AwsCredentials> {
    let tag = |name: &str| {
        xml_tag(xml, name).ok_or_else(|| anyhow::anyhow!("sts response is missing {name}"))
    };
    Ok(AwsCredentials {
        key_id: tag("AccessKeyId")?.to_owned(),
        secret_key: tag("SecretAccessKey")?.to_owned(),
        session_token: Some(tag("SessionToken")?.to_owned()),
        expiry: Some(parse_expiry(tag("Expiration")?)?),
    })
}

fn xml_tag<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let start = xml.find(&format!("<{name}>"))? + name.len() + 2;
    let len = xml[start..].find(&format!("</{name}>"))?;
    Some(xml[start..start + len].trim())
}

fn parse_expiry(s: &str) -> anyhow::Result<SystemTime> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(SystemTime::from)
        .map_err(|e| anyhow::anyhow!("bad credential expiration {s:?}: {e}"))
}

#[cfg(test)]
pub struct FixedSource(pub AwsCredentials);

#[cfg(test)]
#[async_trait::async_trait]
impl CredentialSource for FixedSource {
    async fn fetch(&self) -> anyhow::Result<AwsCredentials> {
        Ok(self.0.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    use axum::Router;
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::{get, post, put};

    const POD_IDENTITY_JSON: &str = r#"{
        "AccessKeyId": "ASIAPODIDENTITY",
        "SecretAccessKey": "pod-secret",
        "Token": "pod-session-token",
        "AccountId": "123456789012",
        "Expiration": "2026-10-21T12:00:00Z"
    }"#;

    fn chain(vars: &[(&str, &str)]) -> DefaultChain {
        let vars: HashMap<String, String> =
            vars.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
        DefaultChain::with_env(Arc::new(move |k: &str| vars.get(k).cloned())).unwrap()
    }

    async fn serve(app: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    }

    fn at(rfc3339: &str) -> SystemTime {
        parse_expiry(rfc3339).unwrap()
    }

    #[test]
    fn parses_pod_identity_response() {
        let c = parse_json_credentials(POD_IDENTITY_JSON).unwrap();
        assert_eq!(c.key_id, "ASIAPODIDENTITY");
        assert_eq!(c.secret_key, "pod-secret");
        assert_eq!(c.session_token.as_deref(), Some("pod-session-token"));
        assert_eq!(c.expiry, Some(at("2026-10-21T12:00:00Z")));
    }

    #[test]
    fn rejects_bad_credential_responses_without_echoing_them() {
        let err = parse_json_credentials(r#"{"AccessKeyId": "AK", "Secret": "leak-me"}"#)
            .unwrap_err()
            .to_string();
        assert!(!err.contains("leak-me"), "{err}");
        let imds_failure = r#"{"Code": "Failure", "AccessKeyId": "A", "SecretAccessKey": "S"}"#;
        assert!(parse_json_credentials(imds_failure).is_err());
        let bad_expiry = r#"{"AccessKeyId": "A", "SecretAccessKey": "S", "Expiration": "soon"}"#;
        assert!(parse_json_credentials(bad_expiry).is_err());
    }

    #[test]
    fn parses_sts_web_identity_response() {
        let xml = "<AssumeRoleWithWebIdentityResponse><AssumeRoleWithWebIdentityResult>\
            <Credentials><AccessKeyId>ASIASTS</AccessKeyId>\
            <SecretAccessKey>sts-secret</SecretAccessKey>\
            <SessionToken>sts-token</SessionToken>\
            <Expiration>2026-10-21T13:00:00Z</Expiration></Credentials>\
            </AssumeRoleWithWebIdentityResult></AssumeRoleWithWebIdentityResponse>";
        let c = parse_sts_credentials(xml).unwrap();
        assert_eq!(c.key_id, "ASIASTS");
        assert_eq!(c.secret_key, "sts-secret");
        assert_eq!(c.session_token.as_deref(), Some("sts-token"));
        assert_eq!(c.expiry, Some(at("2026-10-21T13:00:00Z")));
        assert!(parse_sts_credentials("<Credentials></Credentials>").is_err());
    }

    #[test]
    fn container_uri_allows_https_loopback_and_aws_endpoints_only() {
        for ok in [
            "https://creds.example.com/v1",
            "http://127.0.0.1:9911/creds",
            "http://localhost/creds",
            "http://169.254.170.23/v1/credentials",
            "http://[fd00:ec2::23]/v1/credentials",
            "http://[::1]:80/creds",
        ] {
            let c = chain(&[("AWS_CONTAINER_CREDENTIALS_FULL_URI", ok)]);
            assert!(c.container_uri().unwrap().is_some(), "should allow {ok}");
        }
        for bad in ["http://example.com/creds", "http://169.254.169.254/creds", "ftp://127.0.0.1/"]
        {
            let c = chain(&[("AWS_CONTAINER_CREDENTIALS_FULL_URI", bad)]);
            assert!(c.container_uri().is_err(), "should reject {bad}");
        }
        let c = chain(&[("AWS_CONTAINER_CREDENTIALS_RELATIVE_URI", "/v2/credentials/abc")]);
        assert_eq!(
            c.container_uri().unwrap().unwrap().as_str(),
            "http://169.254.170.2/v2/credentials/abc"
        );
        assert!(chain(&[]).container_uri().unwrap().is_none());
    }

    #[tokio::test]
    async fn env_keys_win_and_carry_the_session_token() {
        let c = chain(&[
            ("AWS_ACCESS_KEY_ID", "AKENV"),
            ("AWS_SECRET_ACCESS_KEY", "env-secret"),
            ("AWS_SESSION_TOKEN", "env-token"),
            ("AWS_CONTAINER_CREDENTIALS_FULL_URI", "http://127.0.0.1:1/unused"),
        ]);
        let creds = c.fetch().await.unwrap();
        assert_eq!(creds.key_id, "AKENV");
        assert_eq!(creds.session_token.as_deref(), Some("env-token"));
        assert_eq!(creds.expiry, None);
    }

    #[tokio::test]
    async fn pod_identity_endpoint_is_called_with_the_rotating_token_file() {
        let app = Router::new().route(
            "/v1/credentials",
            get(|headers: HeaderMap| async move {
                if headers.get("authorization").and_then(|v| v.to_str().ok()) == Some("pod-jwt") {
                    POD_IDENTITY_JSON.into_response()
                } else {
                    (StatusCode::UNAUTHORIZED, "bad token").into_response()
                }
            }),
        );
        let base = serve(app).await;
        let dir = tempfile::tempdir().unwrap();
        let token_path = dir.path().join("eks-pod-identity-token");
        std::fs::write(&token_path, "pod-jwt\n").unwrap();
        let uri = format!("{base}/v1/credentials");
        let c = chain(&[
            ("AWS_CONTAINER_CREDENTIALS_FULL_URI", uri.as_str()),
            ("AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE", token_path.to_str().unwrap()),
        ]);
        let creds = c.fetch().await.unwrap();
        assert_eq!(creds.key_id, "ASIAPODIDENTITY");
        assert_eq!(creds.session_token.as_deref(), Some("pod-session-token"));

        std::fs::write(&token_path, "rotated").unwrap();
        assert!(c.fetch().await.is_err(), "a rotated token must be re-read, not cached");
    }

    #[tokio::test]
    async fn web_identity_exchanges_the_token_at_sts() {
        let app = Router::new().route(
            "/",
            post(|body: String| async move {
                if body.contains("Action=AssumeRoleWithWebIdentity")
                    && body.contains("WebIdentityToken=irsa-jwt")
                    && body.contains("RoleArn=arn%3Aaws%3Aiam%3A%3A123456789012%3Arole%2Fworker")
                {
                    "<Response><Credentials><AccessKeyId>ASIAIRSA</AccessKeyId>\
                     <SecretAccessKey>irsa-secret</SecretAccessKey>\
                     <SessionToken>irsa-token</SessionToken>\
                     <Expiration>2026-10-21T13:00:00Z</Expiration></Credentials></Response>"
                        .into_response()
                } else {
                    (StatusCode::FORBIDDEN, "<Error><Message>denied</Message></Error>")
                        .into_response()
                }
            }),
        );
        let base = serve(app).await;
        let dir = tempfile::tempdir().unwrap();
        let token_path = dir.path().join("token");
        std::fs::write(&token_path, "irsa-jwt\n").unwrap();
        let sts = format!("{base}/");
        let c = chain(&[
            ("AWS_WEB_IDENTITY_TOKEN_FILE", token_path.to_str().unwrap()),
            ("AWS_ROLE_ARN", "arn:aws:iam::123456789012:role/worker"),
            ("AWS_ENDPOINT_URL_STS", sts.as_str()),
        ]);
        let creds = c.fetch().await.unwrap();
        assert_eq!(creds.key_id, "ASIAIRSA");
        assert_eq!(creds.session_token.as_deref(), Some("irsa-token"));
    }

    #[tokio::test]
    async fn imds_v2_fetches_a_token_then_the_role_credentials() {
        let app = Router::new()
            .route(
                "/latest/api/token",
                put(|headers: HeaderMap| async move {
                    if headers.contains_key("x-aws-ec2-metadata-token-ttl-seconds") {
                        "imds-session".into_response()
                    } else {
                        StatusCode::BAD_REQUEST.into_response()
                    }
                }),
            )
            .route(
                "/latest/meta-data/iam/security-credentials/",
                get(|headers: HeaderMap| async move {
                    if headers.get("x-aws-ec2-metadata-token").is_some_and(|v| v == "imds-session")
                    {
                        "node-role\n".into_response()
                    } else {
                        StatusCode::UNAUTHORIZED.into_response()
                    }
                }),
            )
            .route(
                "/latest/meta-data/iam/security-credentials/node-role",
                get(|| async {
                    r#"{"Code": "Success", "Type": "AWS-HMAC", "AccessKeyId": "ASIAIMDS",
                        "SecretAccessKey": "imds-secret", "Token": "imds-token",
                        "Expiration": "2026-10-21T14:00:00Z"}"#
                }),
            );
        let base = serve(app).await;
        let c = chain(&[("AWS_EC2_METADATA_SERVICE_ENDPOINT", base.as_str())]);
        let creds = c.fetch().await.unwrap();
        assert_eq!(creds.key_id, "ASIAIMDS");
        assert_eq!(creds.session_token.as_deref(), Some("imds-token"));

        let disabled = chain(&[("AWS_EC2_METADATA_DISABLED", "true")]);
        assert!(disabled.fetch().await.is_err());
    }

    struct Scripted {
        calls: AtomicUsize,
        fail_from: usize,
        expiry: SystemTime,
    }

    #[async_trait::async_trait]
    impl CredentialSource for Scripted {
        async fn fetch(&self) -> anyhow::Result<AwsCredentials> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            anyhow::ensure!(n < self.fail_from, "endpoint down");
            Ok(AwsCredentials {
                key_id: format!("ASIA{n}"),
                secret_key: "s".to_owned(),
                session_token: Some(format!("token{n}")),
                expiry: Some(self.expiry),
            })
        }
    }

    fn cache_at(fail_from: usize) -> (AmbientAws, Arc<AtomicU64>) {
        let t0 = at("2026-10-21T12:00:00Z");
        let offset = Arc::new(AtomicU64::new(0));
        let clock_offset = offset.clone();
        let source = Scripted {
            calls: AtomicUsize::new(0),
            fail_from,
            expiry: t0 + Duration::from_hours(1),
        };
        let now = Arc::new(move || t0 + Duration::from_secs(clock_offset.load(Ordering::SeqCst)));
        (AmbientAws::new(Box::new(source), now), offset)
    }

    #[tokio::test]
    async fn refreshes_temporary_credentials_before_they_expire() {
        let (ambient, offset) = cache_at(usize::MAX);
        assert_eq!(ambient.credentials().await.unwrap().key_id, "ASIA1");
        offset.store(50 * 60, Ordering::SeqCst);
        assert_eq!(ambient.credentials().await.unwrap().key_id, "ASIA1", "still fresh: cached");
        offset.store(56 * 60, Ordering::SeqCst);
        assert_eq!(ambient.credentials().await.unwrap().key_id, "ASIA2", "inside the margin");
    }

    #[tokio::test]
    async fn failed_refresh_serves_cached_until_actual_expiry() {
        let (ambient, offset) = cache_at(2);
        assert_eq!(ambient.credentials().await.unwrap().key_id, "ASIA1");
        offset.store(58 * 60, Ordering::SeqCst);
        assert_eq!(ambient.credentials().await.unwrap().key_id, "ASIA1");
        offset.store(61 * 60, Ordering::SeqCst);
        assert!(ambient.credentials().await.is_err(), "expired credentials are never served");
    }

    #[tokio::test]
    async fn long_term_credentials_are_fetched_once() {
        let source = FixedSource(AwsCredentials::new("AKIDLONG", "s"));
        let ambient = AmbientAws::new(Box::new(source), Arc::new(SystemTime::now));
        assert_eq!(ambient.credentials().await.unwrap(), AwsCredentials::new("AKIDLONG", "s"));
        assert_eq!(ambient.credentials().await.unwrap().key_id, "AKIDLONG");
    }

    #[test]
    fn debug_redacts_secrets() {
        let mut c = AwsCredentials::new("AKID", "hunter2");
        c.session_token = Some("tok-secret".to_owned());
        let shown = format!("{c:?}");
        assert!(!shown.contains("hunter2"), "{shown}");
        assert!(!shown.contains("tok-secret"), "{shown}");
    }
}
