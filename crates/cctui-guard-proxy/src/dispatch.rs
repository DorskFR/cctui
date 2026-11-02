//! The pod's dispatch identity: the machine key redeemed once at startup for
//! the projected `cctui`-audience token, held here and never by the worker.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

const REDEEM_PATH: &str = "/api/v1/dispatch/redeem";
const REDEEM_ATTEMPTS: u32 = 5;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerTarget {
    pub base_url: String,
    pub https: bool,
    pub host: String,
    pub port: u16,
}

impl ServerTarget {
    pub fn parse(url: &str) -> anyhow::Result<Self> {
        let url = url.trim().trim_end_matches('/');
        let (https, rest) = if let Some(rest) = url.strip_prefix("https://") {
            (true, rest)
        } else if let Some(rest) = url.strip_prefix("http://") {
            (false, rest)
        } else {
            anyhow::bail!("server URL `{url}` must be http:// or https://");
        };
        let authority = rest.split('/').next().unwrap_or_default();
        anyhow::ensure!(
            !authority.is_empty() && !authority.contains('@') && !authority.contains('['),
            "server URL `{url}` has no usable host"
        );
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) => (
                host,
                port.parse::<u16>().map_err(|_| anyhow::anyhow!("bad port in `{url}`"))?,
            ),
            None => (authority, if https { 443 } else { 80 }),
        };
        Ok(Self { base_url: url.to_owned(), https, host: host.to_ascii_lowercase(), port })
    }

    pub fn matches(&self, host: &str, port: u16) -> bool {
        port == self.port && host.eq_ignore_ascii_case(&self.host)
    }
}

#[derive(Serialize)]
struct RedeemRequest<'a> {
    dispatch_id: &'a str,
    token: &'a str,
}

#[derive(Deserialize)]
struct RedeemResponse {
    machine_key: String,
}

#[derive(Deserialize)]
struct ApiError {
    error: String,
}

pub struct DispatchConfig {
    pub dispatch_id: String,
    pub server: ServerTarget,
    pub token_path: PathBuf,
}

pub enum Outcome {
    Redeemed { machine_key: String },
    Failed(String),
}

pub struct DispatchIdentity {
    pub config: DispatchConfig,
    pub outcome: Outcome,
    http: reqwest::Client,
}

impl DispatchIdentity {
    /// Redeems once; a refusal is final, an unreachable server is retried a
    /// few times before the pod is failed with the reason.
    pub async fn redeem(config: DispatchConfig) -> Self {
        crate::inject::install_crypto();
        let http = reqwest::Client::new();
        let outcome = match redeem_with_retry(&http, &config).await {
            Ok(machine_key) => {
                tracing::info!(dispatch = %config.dispatch_id, "dispatch identity redeemed");
                Outcome::Redeemed { machine_key }
            }
            Err(reason) => {
                tracing::error!(dispatch = %config.dispatch_id, %reason, "dispatch redeem failed");
                Outcome::Failed(reason)
            }
        };
        Self { config, outcome, http }
    }

    pub fn machine_key(&self) -> Option<&str> {
        match &self.outcome {
            Outcome::Redeemed { machine_key } => Some(machine_key),
            Outcome::Failed(_) => None,
        }
    }

    /// The worker's payload env as the server returns it, or the reason there
    /// is none: `(status, JSON body)`.
    pub async fn env(&self) -> (u16, String) {
        let machine_key = match &self.outcome {
            Outcome::Redeemed { machine_key } => machine_key,
            Outcome::Failed(reason) => return (503, error_body(reason)),
        };
        let url = format!(
            "{}/api/v1/dispatch/{}/env",
            self.config.server.base_url, self.config.dispatch_id
        );
        let sent = self
            .http
            .get(&url)
            .bearer_auth(machine_key)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await;
        match sent {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let body = resp.text().await.unwrap_or_default();
                (status, body)
            }
            Err(e) => (502, error_body(&format!("fetching the dispatch env from the server: {e}"))),
        }
    }
}

fn error_body(reason: &str) -> String {
    serde_json::json!({ "error": reason }).to_string()
}

enum Attempt {
    Done(String),
    Retry(String),
    Refused(String),
}

async fn redeem_once(http: &reqwest::Client, config: &DispatchConfig, token: &str) -> Attempt {
    let url = format!("{}{REDEEM_PATH}", config.server.base_url);
    let body = RedeemRequest { dispatch_id: &config.dispatch_id, token };
    let resp = match http.post(&url).json(&body).timeout(REQUEST_TIMEOUT).send().await {
        Ok(resp) => resp,
        Err(e) => return Attempt::Retry(format!("cctui server unreachable at {url}: {e}")),
    };
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if status.is_success() {
        return match serde_json::from_str::<RedeemResponse>(&text) {
            Ok(r) if !r.machine_key.is_empty() => Attempt::Done(r.machine_key),
            _ => Attempt::Refused("the server's redeem answer carries no machine key".to_owned()),
        };
    }
    let reason = serde_json::from_str::<ApiError>(&text).map_or(text, |e| e.error);
    let reason = format!("redeem refused by the cctui server ({status}): {reason}");
    if status.is_server_error() { Attempt::Retry(reason) } else { Attempt::Refused(reason) }
}

async fn redeem_with_retry(
    http: &reqwest::Client,
    config: &DispatchConfig,
) -> Result<String, String> {
    let token = std::fs::read_to_string(&config.token_path).map_err(|e| {
        format!(
            "reading the projected service-account token at {}: {e}",
            config.token_path.display()
        )
    })?;
    let token = token.trim();
    let mut last = String::new();
    for attempt in 0..REDEEM_ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_secs(1 << (attempt - 1))).await;
        }
        match redeem_once(http, config, token).await {
            Attempt::Done(key) => return Ok(key),
            Attempt::Refused(reason) => return Err(reason),
            Attempt::Retry(reason) => {
                tracing::warn!(attempt, %reason, "dispatch redeem failed; retrying");
                last = reason;
            }
        }
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::Json;
    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::routing::{get, post};

    use super::*;

    #[test]
    fn server_urls_parse_to_host_and_port() {
        let t = ServerTarget::parse("http://cctui.cctui.svc:8700/").unwrap();
        assert_eq!((t.https, t.host.as_str(), t.port), (false, "cctui.cctui.svc", 8700));
        assert_eq!(t.base_url, "http://cctui.cctui.svc:8700");
        let t = ServerTarget::parse("https://CCTUI.example.test").unwrap();
        assert_eq!((t.https, t.host.as_str(), t.port), (true, "cctui.example.test", 443));
        assert!(t.matches("cctui.Example.test", 443));
        assert!(!t.matches("cctui.example.test", 80));
        for bad in ["ftp://x", "http://", "http://u@h", "http://h:notaport", "http://[::1]:80"] {
            assert!(ServerTarget::parse(bad).is_err(), "{bad}");
        }
    }

    async fn server(app: axum::Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        base
    }

    fn config(base: &str, token_path: PathBuf) -> DispatchConfig {
        DispatchConfig {
            dispatch_id: "d-1".to_owned(),
            server: ServerTarget::parse(base).unwrap(),
            token_path,
        }
    }

    fn token_file(dir: &tempfile::TempDir) -> PathBuf {
        let path = dir.path().join("cctui-token");
        std::fs::write(&path, "projected.jwt.token\n").unwrap();
        path
    }

    #[tokio::test]
    async fn a_redeemed_key_is_kept_and_used_for_the_env() {
        let app = axum::Router::new()
            .route(
                REDEEM_PATH,
                post(|Json(body): Json<serde_json::Value>| async move {
                    assert_eq!(body["dispatch_id"], "d-1");
                    assert_eq!(body["token"], "projected.jwt.token");
                    Json(serde_json::json!({ "machine_key": "real-key" }))
                }),
            )
            .route(
                "/api/v1/dispatch/d-1/env",
                get(|headers: axum::http::HeaderMap| async move {
                    let auth = headers["authorization"].to_str().unwrap().to_owned();
                    assert_eq!(auth, "Bearer real-key");
                    Json(serde_json::json!({ "env": { "FEATURE_FLAG": "on" } }))
                }),
            );
        let base = server(app).await;
        let dir = tempfile::tempdir().unwrap();
        let identity = DispatchIdentity::redeem(config(&base, token_file(&dir))).await;
        assert_eq!(identity.machine_key(), Some("real-key"));
        let (status, body) = identity.env().await;
        assert_eq!(status, 200);
        assert!(body.contains("FEATURE_FLAG"), "{body}");
    }

    #[tokio::test]
    async fn a_refused_token_fails_at_once_with_the_server_reason() {
        let calls = Arc::new(AtomicUsize::new(0));
        let app = axum::Router::new()
            .route(
                REDEEM_PATH,
                post(|State(calls): State<Arc<AtomicUsize>>| async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::UNAUTHORIZED,
                        Json(serde_json::json!({
                            "error": "dispatch identity rejected: token issuer `x` is not trusted"
                        })),
                    )
                }),
            )
            .with_state(calls.clone());
        let base = server(app).await;
        let dir = tempfile::tempdir().unwrap();
        let identity = DispatchIdentity::redeem(config(&base, token_file(&dir))).await;
        assert!(identity.machine_key().is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 1, "a refusal is not retried");
        let (status, body) = identity.env().await;
        assert_eq!(status, 503);
        assert!(body.contains("401") && body.contains("not trusted"), "{body}");
    }

    #[tokio::test(start_paused = true)]
    async fn an_unreachable_server_fails_after_bounded_retries() {
        let dir = tempfile::tempdir().unwrap();
        let identity =
            DispatchIdentity::redeem(config("http://127.0.0.1:9", token_file(&dir))).await;
        let Outcome::Failed(reason) = &identity.outcome else { panic!("must fail") };
        assert!(reason.contains("unreachable"), "{reason}");
    }

    #[tokio::test]
    async fn a_missing_token_file_names_the_path() {
        let missing = config("http://127.0.0.1:9", "/nonexistent/cctui-token".into());
        let identity = DispatchIdentity::redeem(missing).await;
        let Outcome::Failed(reason) = &identity.outcome else { panic!("must fail") };
        assert!(reason.contains("/nonexistent/cctui-token"), "{reason}");
    }
}
