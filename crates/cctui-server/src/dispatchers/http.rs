//! Forwards a [`DispatchSpec`] to an external dispatcher endpoint over HTTP.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::{DispatchError, DispatchHandle, DispatchSpec, Dispatcher};

pub struct HttpDispatcher {
    id: String,
    url: String,
    token: Option<String>,
    client: reqwest::Client,
}

#[derive(Serialize)]
struct DispatchBody<'a> {
    session_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    timeout_minutes: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reply_url: Option<&'a str>,
    payload: &'a serde_json::Value,
}

#[derive(Deserialize)]
struct DispatchReply {
    handle: String,
    #[serde(default)]
    namespace: Option<String>,
    /// Dispatch outcome (`dispatched` / `deduplicated` / `redispatched`).
    /// Absent from older dispatchers, so default to `None` and let the route
    /// fall back (CCT-207).
    #[serde(default)]
    status: Option<String>,
}

impl HttpDispatcher {
    pub fn new(id: impl Into<String>, url: impl Into<String>, token: Option<String>) -> Self {
        Self { id: id.into(), url: url.into(), token, client: reqwest::Client::new() }
    }
}

#[async_trait]
impl Dispatcher for HttpDispatcher {
    fn id(&self) -> &str {
        &self.id
    }

    async fn dispatch(&self, spec: &DispatchSpec<'_>) -> Result<DispatchHandle, DispatchError> {
        let body = DispatchBody {
            session_id: spec.session_id,
            timeout_minutes: spec.timeout_minutes,
            reply_url: spec.reply_url,
            payload: spec.payload,
        };

        let mut builder = self.client.post(&self.url).json(&body);
        if let Some(token) = &self.token {
            builder = builder.bearer_auth(token);
        }
        let resp = builder
            .send()
            .await
            .map_err(|e| DispatchError::Backend(format!("POST {}: {e}", self.url)))?;

        let status = resp.status();
        if !status.is_success() {
            let detail = resp.text().await.unwrap_or_default();
            let msg = format!("dispatcher returned {status}: {detail}");
            return Err(if status == reqwest::StatusCode::BAD_REQUEST {
                DispatchError::InvalidIntent(msg)
            } else {
                DispatchError::Backend(msg)
            });
        }

        let reply: DispatchReply = resp
            .json()
            .await
            .map_err(|e| DispatchError::Backend(format!("decoding dispatcher reply: {e}")))?;

        Ok(DispatchHandle {
            handle: reply.handle,
            namespace: reply.namespace,
            status: reply.status,
        })
    }
}
