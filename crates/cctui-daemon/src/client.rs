//! HTTP + WS client to a cctui-server.

use cctui_proto::api::{
    DaemonAuthRequest, DaemonAuthResponse, GatewayEnvResponse, SessionImageUploadResponse,
    TokenValidResponse,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::counters::{BandwidthCounters, Subsystem};

#[derive(Debug, Clone)]
pub struct ServerClient {
    base_url: String,
    http: reqwest::Client,
    counters: BandwidthCounters,
}

#[derive(Debug, Serialize)]
struct EnrollBody<'a> {
    hostname: &'a str,
    /// Machine kind. Omitted for `persistent` so older servers that
    /// don't know the field are unaffected; sent as `ephemeral` for worker pods.
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<&'a str>,
}

#[derive(Debug, Deserialize)]
pub struct EnrollResponse {
    pub machine_id: Uuid,
    pub machine_key: String,
}

/// `GET /api/v1/machines/{id}/status`: connectivity snapshot used
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
        Self {
            base_url: base_url.into(),
            http: reqwest::Client::new(),
            counters: BandwidthCounters::new(),
        }
    }

    /// Attach shared bandwidth counters so blob/image PUT bodies are accounted
    /// under [`Subsystem::BlobPut`].
    #[must_use]
    pub fn with_counters(mut self, counters: BandwidthCounters) -> Self {
        self.counters = counters;
        self
    }

    #[must_use]
    pub fn counters(&self) -> BandwidthCounters {
        self.counters.clone()
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
    /// `sessions.account_id` binding. Called by the claude adapter's
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
    /// resolves at the gateway. `token_hash` is the sha256 hex of the
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

    /// Upload an agent-posted image blob the daemon detected as a
    /// marker in an assistant message. Raw bytes body; the server sniffs the
    /// media type from magic bytes and dedups by sha256. Returns the stored blob
    /// id the caller rewrites into a `cctui-img://<id>` marker.
    pub async fn upload_session_image(
        &self,
        machine_key: &str,
        session_id: &str,
        bytes: Vec<u8>,
        media_type: &str,
    ) -> anyhow::Result<String> {
        let url = format!(
            "{}/api/v1/daemon/sessions/{}/images",
            self.base_url.trim_end_matches('/'),
            session_id,
        );
        let len = bytes.len() as u64;
        let resp = self
            .http
            .post(&url)
            .bearer_auth(machine_key)
            .header(reqwest::header::CONTENT_TYPE, media_type)
            .body(bytes)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("upload_session_image failed ({status}): {text}");
        }
        self.counters.add(Subsystem::BlobPut, len);
        let parsed: SessionImageUploadResponse = resp.json().await?;
        Ok(parsed.image_id)
    }

    /// Upload a content-addressed blob: raw bytes keyed by their
    /// sha256 hex. Idempotent — a re-PUT of an already-stored hash is a cheap
    /// 200/204. `media_type` sets the `Content-Type` when known.
    pub async fn put_blob(
        &self,
        machine_key: &str,
        hash: &str,
        bytes: Vec<u8>,
        media_type: Option<&str>,
    ) -> anyhow::Result<()> {
        let url = format!("{}/api/v1/daemon/blobs/{}", self.base_url.trim_end_matches('/'), hash);
        let len = bytes.len() as u64;
        let mut req = self.http.put(&url).bearer_auth(machine_key).body(bytes);
        if let Some(mt) = media_type {
            req = req.header(reqwest::header::CONTENT_TYPE, mt);
        }
        let resp = req.send().await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("put_blob failed ({status}): {text}");
        }
        self.counters.add(Subsystem::BlobPut, len);
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    async fn serve_put(status: &'static str) -> String {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let _ = sock.read(&mut buf).await.unwrap();
            sock.write_all(status.as_bytes()).await.unwrap();
            sock.flush().await.unwrap();
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn put_blob_accounts_body_bytes_under_blob_put() {
        let url = serve_put("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n").await;
        let counters = BandwidthCounters::new();
        let client = ServerClient::new(url).with_counters(counters.clone());
        client.put_blob("mkey", "abc123", vec![7u8; 512], Some("image/png")).await.unwrap();
        assert_eq!(counters.summary().blob_put, 512);
    }
}
