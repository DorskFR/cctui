//! HTTP + WS client to a cctui-server.

use cctui_proto::api::{DaemonAuthRequest, DaemonAuthResponse};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct ServerClient {
    base_url: String,
    http: reqwest::Client,
}

#[derive(Debug, Serialize)]
struct EnrollBody<'a> {
    hostname: &'a str,
}

#[derive(Debug, Deserialize)]
pub struct EnrollResponse {
    pub machine_id: Uuid,
    pub machine_key: String,
}

impl ServerClient {
    #[must_use]
    pub fn new(base_url: impl Into<String>) -> Self {
        Self { base_url: base_url.into(), http: reqwest::Client::new() }
    }

    pub async fn enroll(&self, user_token: &str, hostname: &str) -> anyhow::Result<EnrollResponse> {
        let url = format!("{}/api/v1/enroll", self.base_url.trim_end_matches('/'));
        let resp = self
            .http
            .post(&url)
            .bearer_auth(user_token)
            .json(&EnrollBody { hostname })
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("enroll failed ({status}): {text}");
        }
        Ok(resp.json().await?)
    }

    pub async fn daemon_auth(&self, machine_key: &str) -> anyhow::Result<DaemonAuthResponse> {
        let url = format!("{}/api/v1/daemon/auth", self.base_url.trim_end_matches('/'));
        let resp = self
            .http
            .post(&url)
            .json(&DaemonAuthRequest { machine_key: machine_key.into() })
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("daemon_auth failed ({status}): {text}");
        }
        Ok(resp.json().await?)
    }

    /// Build the daemon WS URL with the machine key as the `token` query
    /// parameter. Caller is responsible for opening the WS connection.
    #[must_use]
    pub fn daemon_ws_url(&self, machine_key: &str) -> String {
        let base = self.base_url.trim_end_matches('/');
        let ws_base = base.replacen("http://", "ws://", 1).replacen("https://", "wss://", 1);
        format!("{ws_base}/api/v1/daemon/ws?token={machine_key}")
    }
}
