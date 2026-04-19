//! REST client for `cctui-server`. Port of `channel/src/bridge.ts`.
//!
//! Error handling mirrors the TS: "fire and forget" calls swallow errors,
//! best-effort polls default to empty results, and hard-required calls bubble
//! errors to the caller.

use std::sync::Arc;
use std::time::Duration;

use reqwest::{Client, StatusCode};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use crate::types::{
    ChannelRegisterResponse, PendingMessage, PermissionRequest, PreToolUsePayload,
    SessionPollResponse, StreamerEvent,
};
use cctui_proto::api::{
    ManifestEntry, ManifestPostRequest, RegisterRequest, RegisterResponse, SkillIndexEntry,
};

#[derive(Debug, Clone, Copy)]
pub enum ArchiveState {
    Present,
    Absent,
}

#[derive(Debug, Clone, Copy)]
pub struct PolicyVerdict {
    pub decision: Decision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny,
}

#[derive(Clone)]
pub struct Bridge {
    inner: Arc<Inner>,
}

struct Inner {
    base_url: String,
    token: String,
    http: Client,
}

impl Bridge {
    #[must_use]
    pub fn new(base_url: impl Into<String>, token: impl Into<String>) -> Self {
        let http = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("reqwest client build");
        Self { inner: Arc::new(Inner { base_url: base_url.into(), token: token.into(), http }) }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.inner.base_url, path)
    }

    fn auth(&self) -> String {
        format!("Bearer {}", self.inner.token)
    }

    pub async fn register_session(
        &self,
        req: &RegisterRequest,
    ) -> anyhow::Result<RegisterResponse> {
        let res = self
            .inner
            .http
            .post(self.url("/api/v1/sessions/register"))
            .header("Authorization", self.auth())
            .json(req)
            .send()
            .await?;
        if !res.status().is_success() {
            let status = res.status();
            let text = res.text().await.unwrap_or_default();
            anyhow::bail!("register failed: {status} {text}");
        }
        Ok(res.json().await?)
    }

    pub async fn post_event(&self, session_id: &str, event: &StreamerEvent) {
        let _ = self
            .inner
            .http
            .post(self.url(&format!("/api/v1/events/{session_id}")))
            .header("Authorization", self.auth())
            .json(event)
            .send()
            .await;
    }

    pub async fn post_transcript_line(&self, session_id: &str, line: &str) {
        let res = self
            .inner
            .http
            .post(self.url(&format!("/api/v1/sessions/{session_id}/transcript")))
            .header("Authorization", self.auth())
            .json(&json!({ "line": line }))
            .send()
            .await;
        if let Ok(res) = res
            && !res.status().is_success()
        {
            tracing::error!(status = %res.status(), "transcript ingest failed");
        }
    }

    pub async fn check_policy(&self, payload: &PreToolUsePayload) -> PolicyVerdict {
        let res = self
            .inner
            .http
            .post(self.url("/api/v1/check"))
            .header("Authorization", self.auth())
            .json(payload)
            .send()
            .await;
        let Ok(res) = res else {
            return PolicyVerdict { decision: Decision::Allow };
        };
        if !res.status().is_success() {
            return PolicyVerdict { decision: Decision::Allow };
        }
        #[derive(Deserialize)]
        struct Verdict {
            decision: String,
        }
        match res.json::<Verdict>().await {
            Ok(v) if v.decision == "deny" => PolicyVerdict { decision: Decision::Deny },
            _ => PolicyVerdict { decision: Decision::Allow },
        }
    }

    pub async fn fetch_pending_messages(&self, session_id: &str) -> Vec<PendingMessage> {
        let res = self
            .inner
            .http
            .get(self.url(&format!("/api/v1/sessions/{session_id}/messages/pending")))
            .header("Authorization", self.auth())
            .send()
            .await;
        let Ok(res) = res else { return Vec::new() };
        if !res.status().is_success() {
            return Vec::new();
        }
        res.json::<Vec<PendingMessage>>().await.unwrap_or_default()
    }

    pub async fn register_channel(
        &self,
        machine_id: &str,
        ppid: u32,
        cwd: &str,
    ) -> anyhow::Result<ChannelRegisterResponse> {
        let res = self
            .inner
            .http
            .post(self.url("/api/v1/channels/register"))
            .header("Authorization", self.auth())
            .json(&json!({ "machine_id": machine_id, "ppid": ppid, "cwd": cwd }))
            .send()
            .await?;
        if !res.status().is_success() {
            let status = res.status();
            let text = res.text().await.unwrap_or_default();
            anyhow::bail!("channel register failed: {status} {text}");
        }
        Ok(res.json().await?)
    }

    pub async fn poll_session(&self, channel_id: &str) -> anyhow::Result<SessionPollResponse> {
        let res = self
            .inner
            .http
            .get(self.url(&format!("/api/v1/channels/{channel_id}/session")))
            .header("Authorization", self.auth())
            .send()
            .await?;
        if !res.status().is_success() {
            anyhow::bail!("poll session failed: {}", res.status());
        }
        Ok(res.json().await?)
    }

    pub async fn submit_permission_request(
        &self,
        session_id: &str,
        req: &PermissionRequest,
    ) -> anyhow::Result<()> {
        let res = self
            .inner
            .http
            .post(self.url(&format!("/api/v1/sessions/{session_id}/permission/request")))
            .header("Authorization", self.auth())
            .json(req)
            .send()
            .await?;
        if !res.status().is_success() {
            let status = res.status();
            let text = res.text().await.unwrap_or_default();
            anyhow::bail!("submit_permission_request failed: {status} {text}");
        }
        Ok(())
    }

    /// Poll until the decision is made or `timeout` elapses. Returns `"allow"` / `"deny"`
    /// on decision, `None` on timeout.
    pub async fn poll_permission_decision(
        &self,
        session_id: &str,
        request_id: &str,
        timeout: Duration,
        interval: Duration,
    ) -> Option<String> {
        let deadline = tokio::time::Instant::now() + timeout;
        while tokio::time::Instant::now() < deadline {
            let url = self
                .url(&format!("/api/v1/sessions/{session_id}/permission/decision/{request_id}"));
            let res = self.inner.http.get(&url).header("Authorization", self.auth()).send().await;
            #[derive(Deserialize)]
            struct Body {
                status: String,
                #[serde(default)]
                behavior: Option<String>,
            }
            if let Ok(res) = res
                && res.status().is_success()
                && let Ok(body) = res.json::<Body>().await
                && body.status == "decided"
            {
                return body.behavior;
            }
            tokio::time::sleep(interval).await;
        }
        None
    }

    pub async fn head_archive(
        &self,
        project_dir: &str,
        session_id: &str,
        sha256: &str,
    ) -> anyhow::Result<ArchiveState> {
        let url = self.url(&format!(
            "/api/v1/archive/{}/{}?sha256={sha256}",
            urlencode(project_dir),
            urlencode(session_id)
        ));
        let res = self.inner.http.head(&url).header("Authorization", self.auth()).send().await?;
        match res.status() {
            StatusCode::NO_CONTENT => Ok(ArchiveState::Present),
            StatusCode::NOT_FOUND => Ok(ArchiveState::Absent),
            other => anyhow::bail!("HEAD archive: unexpected {other}"),
        }
    }

    pub async fn put_archive(
        &self,
        project_dir: &str,
        session_id: &str,
        abs_path: &std::path::Path,
        sha256: &str,
    ) -> anyhow::Result<()> {
        let mut file = tokio::fs::File::open(abs_path).await?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf).await?;
        let url = self.url(&format!(
            "/api/v1/archive/{}/{}",
            urlencode(project_dir),
            urlencode(session_id)
        ));
        let res = self
            .inner
            .http
            .put(&url)
            .header("Authorization", self.auth())
            .header("X-CCTUI-SHA256", sha256)
            .header("Content-Type", "application/octet-stream")
            .body(buf)
            .send()
            .await?;
        if !res.status().is_success() {
            let status = res.status();
            let text = res.text().await.unwrap_or_default();
            anyhow::bail!("PUT archive failed: {status} {text}");
        }
        Ok(())
    }

    /// POST the machine's current expected-files manifest (CCT-68).
    pub async fn post_manifest(&self, entries: &[ManifestEntry]) -> anyhow::Result<()> {
        let body = ManifestPostRequest { entries: entries.to_vec() };
        let res = self
            .inner
            .http
            .post(self.url("/api/v1/archive/manifest"))
            .header("Authorization", self.auth())
            .json(&body)
            .send()
            .await?;
        if !res.status().is_success() {
            let status = res.status();
            let text = res.text().await.unwrap_or_default();
            anyhow::bail!("POST manifest failed: {status} {text}");
        }
        Ok(())
    }

    pub async fn get_skill_index(&self) -> anyhow::Result<Vec<SkillIndexEntry>> {
        let res = self
            .inner
            .http
            .get(self.url("/api/v1/skills/index"))
            .header("Authorization", self.auth())
            .send()
            .await?;
        if !res.status().is_success() {
            let status = res.status();
            let text = res.text().await.unwrap_or_default();
            anyhow::bail!("skill index failed: {status} {text}");
        }
        Ok(res.json().await?)
    }

    pub async fn get_skill_bundle(&self, name: &str) -> anyhow::Result<Vec<u8>> {
        let res = self
            .inner
            .http
            .get(self.url(&format!("/api/v1/skills/{}", urlencode(name))))
            .header("Authorization", self.auth())
            .send()
            .await?;
        if !res.status().is_success() {
            let status = res.status();
            let text = res.text().await.unwrap_or_default();
            anyhow::bail!("skill get failed: {status} {text}");
        }
        Ok(res.bytes().await?.to_vec())
    }
}

/// Minimal percent-encoder for path segments (matches the TS `encodeURIComponent` usage).
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}
