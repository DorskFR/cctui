use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde::de::DeserializeOwned;

use super::jwt::{Jwks, Rejection};

pub const ISSUERS_ENV: &str = "CCTUI_DISPATCH_TRUSTED_ISSUERS";
const FRESH_FOR: Duration = Duration::from_secs(600);
/// An unknown `kid` refetches (key rotation), but never more often than this.
const REFETCH_FLOOR: Duration = Duration::from_secs(30);
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct TrustedIssuer {
    issuer: String,
    #[serde(default)]
    jwks_uri: Option<String>,
    #[serde(default)]
    jwks_file: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Source {
    Discovery(String),
    Uri(String),
    File(PathBuf),
}

struct Cached {
    jwks: Jwks,
    at: Instant,
}

pub struct Issuers {
    trusted: HashMap<String, Source>,
    cache: Mutex<HashMap<String, Cached>>,
}

impl Issuers {
    pub fn parse(json: &str) -> Result<Self, String> {
        let list: Vec<TrustedIssuer> = if json.trim().is_empty() {
            Vec::new()
        } else {
            serde_json::from_str(json).map_err(|e| format!("{ISSUERS_ENV}: {e}"))?
        };
        let mut trusted = HashMap::new();
        for t in list {
            let issuer = t.issuer.trim().to_owned();
            if issuer.is_empty() {
                return Err(format!("{ISSUERS_ENV}: an entry has an empty issuer"));
            }
            let source = match (t.jwks_uri, t.jwks_file) {
                (Some(_), Some(_)) => {
                    return Err(format!(
                        "{ISSUERS_ENV}: `{issuer}` sets both jwks_uri and jwks_file"
                    ));
                }
                (Some(uri), None) => Source::Uri(uri),
                (None, Some(file)) => Source::File(file),
                (None, None) => Source::Discovery(format!(
                    "{}/.well-known/openid-configuration",
                    issuer.trim_end_matches('/')
                )),
            };
            if trusted.insert(issuer.clone(), source).is_some() {
                return Err(format!("{ISSUERS_ENV}: `{issuer}` is listed twice"));
            }
        }
        Ok(Self { trusted, cache: Mutex::new(HashMap::new()) })
    }

    pub fn from_env() -> Result<Self, String> {
        Self::parse(&std::env::var(ISSUERS_ENV).unwrap_or_default())
    }

    pub async fn keys(
        &self,
        http: &reqwest::Client,
        issuer: &str,
        kid: Option<&str>,
    ) -> Result<Jwks, Rejection> {
        let source =
            self.trusted.get(issuer).ok_or_else(|| Rejection::UntrustedIssuer(issuer.to_owned()))?;
        if let Some(jwks) = self.cached(issuer, kid) {
            return Ok(jwks);
        }
        let jwks = fetch(http, source)
            .await
            .map_err(|error| Rejection::Keys { issuer: issuer.to_owned(), error })?;
        self.cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(issuer.to_owned(), Cached { jwks: jwks.clone(), at: Instant::now() });
        Ok(jwks)
    }

    fn cached(&self, issuer: &str, kid: Option<&str>) -> Option<Jwks> {
        let cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        let entry = cache.get(issuer)?;
        let age = entry.at.elapsed();
        let knows_kid = kid.is_none_or(|k| entry.jwks.has(k));
        (age < REFETCH_FLOOR || (age < FRESH_FOR && knows_kid)).then(|| entry.jwks.clone())
    }
}

async fn fetch(http: &reqwest::Client, source: &Source) -> Result<Jwks, String> {
    match source {
        Source::File(path) => {
            let raw =
                tokio::fs::read(path).await.map_err(|e| format!("{}: {e}", path.display()))?;
            serde_json::from_slice(&raw).map_err(|e| format!("{}: {e}", path.display()))
        }
        Source::Uri(uri) => get_json(http, uri).await,
        Source::Discovery(url) => {
            #[derive(Deserialize)]
            struct Discovery {
                jwks_uri: String,
            }
            let discovery: Discovery = get_json(http, url).await?;
            get_json(http, &discovery.jwks_uri).await
        }
    }
}

async fn get_json<T: DeserializeOwned>(http: &reqwest::Client, url: &str) -> Result<T, String> {
    let resp = http
        .get(url)
        .timeout(FETCH_TIMEOUT)
        .send()
        .await
        .map_err(|e| format!("GET {url}: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("GET {url}: HTTP {status}"));
    }
    resp.json::<T>().await.map_err(|e| format!("GET {url}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch_identity::jwt::test_keys::Signer;

    fn jwks_json(signer: &Signer) -> String {
        let jwk = signer.jwk();
        serde_json::json!({ "keys": [{
            "kty": jwk.kty, "kid": jwk.kid, "crv": jwk.crv, "x": jwk.x, "y": jwk.y, "use": "sig"
        }] })
        .to_string()
    }

    fn http() -> reqwest::Client {
        let _ = rustls::crypto::ring::default_provider().install_default();
        reqwest::Client::new()
    }

    #[test]
    fn config_lists_trusted_issuers_with_their_key_source() {
        let issuers = Issuers::parse(
            r#"[
                {"issuer": "https://oidc.example.test/id/a"},
                {"issuer": "https://kubernetes.default.svc", "jwks_file": "/etc/cctui/jwks.json"},
                {"issuer": "https://b.example.test", "jwks_uri": "https://b.example.test/keys"}
            ]"#,
        )
        .unwrap();
        assert_eq!(
            issuers.trusted["https://oidc.example.test/id/a"],
            Source::Discovery(
                "https://oidc.example.test/id/a/.well-known/openid-configuration".into()
            )
        );
        assert_eq!(
            issuers.trusted["https://kubernetes.default.svc"],
            Source::File("/etc/cctui/jwks.json".into())
        );
        assert_eq!(
            issuers.trusted["https://b.example.test"],
            Source::Uri("https://b.example.test/keys".into())
        );
        assert!(Issuers::parse("").unwrap().trusted.is_empty(), "unset trusts nobody");
    }

    #[test]
    fn ambiguous_or_broken_config_is_an_error() {
        for bad in [
            r#"[{"issuer": "https://a", "jwks_uri": "u", "jwks_file": "f"}]"#,
            r#"[{"issuer": "https://a"}, {"issuer": "https://a"}]"#,
            r#"[{"issuer": " "}]"#,
            r#"[{"issuer": "https://a", "audience": "x"}]"#,
            "not json",
        ] {
            assert!(Issuers::parse(bad).is_err(), "{bad}");
        }
    }

    #[tokio::test]
    async fn an_untrusted_issuer_is_refused_without_fetching() {
        let issuers = Issuers::parse(r#"[{"issuer": "https://a.example.test"}]"#).unwrap();
        let err = issuers.keys(&http(), "https://evil.example.test", None).await.unwrap_err();
        assert_eq!(err, Rejection::UntrustedIssuer("https://evil.example.test".into()));
    }

    #[tokio::test]
    async fn a_jwks_file_is_read_and_reread_for_a_rotated_kid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jwks.json");
        let first = Signer::new("k1");
        std::fs::write(&path, jwks_json(&first)).unwrap();
        let config = serde_json::json!([{ "issuer": "https://k3s.test", "jwks_file": path }]);
        let issuers = Issuers::parse(&config.to_string()).unwrap();

        let jwks = issuers.keys(&http(), "https://k3s.test", Some("k1")).await.unwrap();
        assert!(jwks.has("k1"));

        let rotated = Signer::new("k2");
        std::fs::write(&path, jwks_json(&rotated)).unwrap();
        let jwks = issuers.keys(&http(), "https://k3s.test", Some("k2")).await.unwrap();
        assert!(!jwks.has("k2"), "inside the refetch floor the cached set answers");

        issuers.cache.lock().unwrap().get_mut("https://k3s.test").unwrap().at =
            Instant::now().checked_sub(REFETCH_FLOOR * 2).unwrap();
        let jwks = issuers.keys(&http(), "https://k3s.test", Some("k2")).await.unwrap();
        assert!(jwks.has("k2"), "past the floor an unknown kid refetches");
    }

    #[tokio::test]
    async fn discovery_follows_the_issuer_jwks_uri() {
        use axum::routing::get;

        let signer = Signer::new("eks-1");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let jwks_body = jwks_json(&signer);
        let discovery = serde_json::json!({ "issuer": base, "jwks_uri": format!("{base}/keys") });
        let app = axum::Router::new()
            .route(
                "/.well-known/openid-configuration",
                get(move || {
                    let body = discovery.to_string();
                    async move { body }
                }),
            )
            .route(
                "/keys",
                get(move || {
                    let body = jwks_body.clone();
                    async move { body }
                }),
            );
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let issuers = Issuers::parse(&serde_json::json!([{ "issuer": base }]).to_string()).unwrap();
        let jwks = issuers.keys(&http(), &base, Some("eks-1")).await.unwrap();
        assert!(jwks.has("eks-1"));
    }
}
