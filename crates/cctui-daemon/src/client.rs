//! HTTP + WS client to a cctui-server.

use cctui_proto::api::{
    DaemonAuthRequest, DaemonAuthResponse, GatewayEnvResponse, TokenValidResponse,
};
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
    /// Machine kind (CCT-183). Omitted for `persistent` so older servers that
    /// don't know the field are unaffected; sent as `ephemeral` for worker pods.
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<&'a str>,
}

#[derive(Debug, Deserialize)]
pub struct EnrollResponse {
    pub machine_id: Uuid,
    pub machine_key: String,
}

/// `GET /api/v1/machines/{id}/status` (CCT-548): connectivity snapshot used
/// by remote enroll to verify the freshly installed daemon actually joined
/// the fleet.
#[derive(Debug, Deserialize)]
pub struct MachineStatus {
    pub machine_id: Uuid,
    pub name: String,
    pub connected: bool,
    pub liveness: String,
    pub revoked: bool,
}

impl ServerClient {
    #[must_use]
    pub fn new(base_url: impl Into<String>) -> Self {
        Self { base_url: base_url.into(), http: reqwest::Client::new() }
    }

    pub async fn enroll(
        &self,
        user_token: &str,
        hostname: &str,
        kind: Option<&str>,
    ) -> anyhow::Result<EnrollResponse> {
        let url = format!("{}/api/v1/enroll", self.base_url.trim_end_matches('/'));
        let resp = self
            .http
            .post(&url)
            .bearer_auth(user_token)
            .json(&EnrollBody { hostname, kind })
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("enroll failed ({status}): {text}");
        }
        Ok(resp.json().await?)
    }

    pub async fn machine_status(
        &self,
        user_token: &str,
        machine_id: Uuid,
    ) -> anyhow::Result<MachineStatus> {
        let url =
            format!("{}/api/v1/machines/{machine_id}/status", self.base_url.trim_end_matches('/'));
        let resp = self.http.get(&url).bearer_auth(user_token).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("machine_status failed ({status}): {text}");
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

    /// Pull a session's gateway-routing env from the server's durable
    /// `sessions.account_id` binding (CCT-460). Called by the claude adapter's
    /// launch chokepoint on every worker (re)launch so the gateway credential is
    /// re-derived from the DB rather than relying on volatile process/in-memory
    /// state surviving a daemon / claude-daemon restart or a session-id rotation.
    pub async fn gateway_env(
        &self,
        machine_key: &str,
        session_id: &str,
    ) -> anyhow::Result<GatewayEnvResponse> {
        let url = format!(
            "{}/api/v1/daemon/sessions/{}/gateway-env",
            self.base_url.trim_end_matches('/'),
            session_id,
        );
        let resp = self.http.get(&url).bearer_auth(machine_key).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("gateway_env failed ({status}): {text}");
        }
        Ok(resp.json().await?)
    }

    /// Ask whether the session token a trusted worker was launched with still
    /// resolves at the gateway (CCT-462). `token_hash` is the sha256 hex of the
    /// token — the token itself never travels on this call. `Err` covers both
    /// network failures and non-200 responses; the caller MUST treat `Err` as
    /// "unknown" (no heal), never as invalid — the heal kill is destructive.
    pub async fn session_token_valid(
        &self,
        machine_key: &str,
        session_id: &str,
        token_hash: &str,
    ) -> anyhow::Result<TokenValidResponse> {
        let url = format!(
            "{}/api/v1/daemon/sessions/{}/token-valid",
            self.base_url.trim_end_matches('/'),
            session_id,
        );
        let resp = self
            .http
            .get(&url)
            .query(&[("hash", token_hash)])
            .bearer_auth(machine_key)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("session_token_valid failed ({status}): {text}");
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
