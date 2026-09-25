//! HTTP + WS client to a cctui-server, dispatcher flavour.
//!
//! A thin enroll wrapper, an auth handshake, and a dial-out WS URL builder,
//! mirroring the daemon's `ServerClient`. The `kind` (`docker`/`kubernetes`/
//! `apple`) is fixed at construction. `enroll` optionally binds a default OAuth
//! account: a dispatch carrying no explicit account routes model traffic through
//! it.

use cctui_proto::api::{DaemonAuthRequest, DaemonAuthResponse};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct ServerClient {
    base_url: String,
    kind: &'static str,
    http: reqwest::Client,
}

#[derive(Debug, Serialize)]
struct EnrollBody<'a> {
    name: &'a str,
    kind: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    account: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider: Option<&'a str>,
}

#[derive(Debug, Deserialize)]
pub struct EnrollResponse {
    pub dispatcher_id: Uuid,
    pub dispatcher_key: String,
}

impl ServerClient {
    #[must_use]
    pub fn new(base_url: impl Into<String>, kind: &'static str) -> Self {
        Self { base_url: base_url.into(), kind, http: reqwest::Client::new() }
    }

    /// Enroll this dispatcher to the caller's account (peer of a machine). The
    /// server mints an id + key the same way as the machine enroll; the key is
    /// persisted to `dispatcher.toml`. `account`/`provider` optionally bind a
    /// default OAuth account for dispatches that name none.
    pub async fn enroll(
        &self,
        user_token: &str,
        name: &str,
        account: Option<&str>,
        provider: Option<&str>,
    ) -> anyhow::Result<EnrollResponse> {
        let url = format!("{}/api/v1/dispatcher/enroll", self.base_url.trim_end_matches('/'));
        let resp = self
            .http
            .post(&url)
            .bearer_auth(user_token)
            .json(&EnrollBody { name, kind: self.kind, account, provider })
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("enroll failed ({status}): {text}");
        }
        Ok(resp.json().await?)
    }

    /// Confirm identity up-front so a misconfiguration fails loudly before the
    /// WS loop starts. Reuses the daemon auth shapes — the server treats an
    /// enrolled dispatcher as a peer of a machine.
    pub async fn dispatcher_auth(&self, key: &str) -> anyhow::Result<DaemonAuthResponse> {
        let url = format!("{}/api/v1/dispatcher/auth", self.base_url.trim_end_matches('/'));
        let resp = self
            .http
            .post(&url)
            .json(&DaemonAuthRequest { machine_key: key.into() })
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("dispatcher_auth failed ({status}): {text}");
        }
        Ok(resp.json().await?)
    }

    #[must_use]
    pub fn dispatcher_ws_url(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        let ws_base = base.replacen("http://", "ws://", 1).replacen("https://", "wss://", 1);
        format!("{ws_base}/api/v1/dispatcher/ws")
    }

    /// WS handshake request carrying the dispatcher key as a Bearer header,
    /// keeping it out of the URL and so out of proxy access logs.
    pub fn dispatcher_ws_request(
        &self,
        key: &str,
    ) -> anyhow::Result<tokio_tungstenite::tungstenite::handshake::client::Request> {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut request = self.dispatcher_ws_url().into_client_request()?;
        request
            .headers_mut()
            .insert(reqwest::header::AUTHORIZATION, format!("Bearer {key}").parse()?);
        Ok(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ws_url_upgrades_scheme_without_a_token() {
        let c = ServerClient::new("https://cctui.example.test/", "docker");
        assert_eq!(c.dispatcher_ws_url(), "wss://cctui.example.test/api/v1/dispatcher/ws");
        let c = ServerClient::new("http://localhost:8700", "kubernetes");
        assert_eq!(c.dispatcher_ws_url(), "ws://localhost:8700/api/v1/dispatcher/ws");
    }

    #[test]
    fn ws_request_sends_key_as_bearer_not_query() {
        let c = ServerClient::new("https://cctui.example.test", "docker");
        let req = c.dispatcher_ws_request("k-123").unwrap();
        assert_eq!(req.uri().query(), None);
        assert!(!req.uri().to_string().contains("k-123"));
        assert_eq!(req.headers()[reqwest::header::AUTHORIZATION], "Bearer k-123");
    }
}
