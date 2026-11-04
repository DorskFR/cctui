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
    /// fall back.
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    queue_position: Option<u32>,
    #[serde(default)]
    exposure: Option<cctui_proto::api::DispatchExposure>,
}

impl HttpDispatcher {
    pub fn new(
        id: impl Into<String>,
        url: impl Into<String>,
        token: Option<String>,
        client: reqwest::Client,
    ) -> Self {
        Self { id: id.into(), url: url.into(), token, client }
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
            queue_position: reply.queue_position,
            exposure: reply.exposure,
        })
    }
}

#[cfg(test)]
mod tests {
    use cctui_proto::api::{DispatchExposure, HumanGate};

    #[test]
    fn a_reply_carries_the_profile_exposure_when_the_dispatcher_reports_it() {
        let reply: super::DispatchReply = serde_json::from_value(serde_json::json!({
            "handle": "job/w-1",
            "exposure": {
                "untrusted_input": true,
                "sensitive_data": true,
                "outward": true,
                "human_gate": "outward_held_for_approval"
            }
        }))
        .unwrap();
        assert_eq!(
            reply.exposure,
            Some(DispatchExposure {
                untrusted_input: true,
                sensitive_data: true,
                outward: true,
                human_gate: Some(HumanGate::OutwardHeldForApproval),
            })
        );
        let older: super::DispatchReply =
            serde_json::from_value(serde_json::json!({ "handle": "job/w-1" })).unwrap();
        assert!(older.exposure.is_none());
    }
}
