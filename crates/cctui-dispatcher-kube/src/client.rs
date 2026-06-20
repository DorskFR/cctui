//! HTTP + WS client to a cctui-server, dispatcher flavour.
//!
//! Mirrors the daemon's `ServerClient` (CCT-248 transport spec): a thin enroll
//! wrapper, an auth handshake, and a dial-out WS URL builder. The dispatcher
//! enroll/auth/ws routes (`/api/v1/dispatcher/{enroll,auth,ws}`) are the
//! server-side surface specced on CCT-248 and land with the server-side rework
//! (CCT-248 parts 2-4); see the deferral note in `lib.rs`.

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
    name: &'a str,
    kind: &'a str,
    /// Optional OAuth account name to bind as this dispatcher's default
    /// (CCT-427). Dispatches with no explicit account route through it.
    #[serde(skip_serializing_if = "Option::is_none")]
    account: Option<&'a str>,
    /// Optional provider hint disambiguating an account name shared across
    /// providers (e.g. an `anthropic` and an `openai` account with the same
    /// name). Recorded alongside the binding.
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
    pub fn new(base_url: impl Into<String>) -> Self {
        Self { base_url: base_url.into(), http: reqwest::Client::new() }
    }

    /// Enroll this dispatcher to the caller's account (peer of a machine,
    /// `kind='kubernetes'`). The server mints an id + key the same way as the
    /// machine enroll; the key is persisted to `dispatcher.toml`.
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
            .json(&EnrollBody { name, kind: "kubernetes", account, provider })
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

    /// Build the dispatcher WS URL with the key as the `token` query parameter.
    #[must_use]
    pub fn dispatcher_ws_url(&self, key: &str) -> String {
        let base = self.base_url.trim_end_matches('/');
        let ws_base = base.replacen("http://", "ws://", 1).replacen("https://", "wss://", 1);
        format!("{ws_base}/api/v1/dispatcher/ws?token={key}")
    }
}
