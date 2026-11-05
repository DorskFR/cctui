//! Forwards a [`DispatchSpec`] to an external dispatcher endpoint over HTTP.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use super::{DispatchError, DispatchHandle, DispatchSpec, Dispatcher};

/// A bearer token read at each dispatch, so a rotation lands on the next one
/// (`cctui_secrets::Secret::subscribe`).
pub type Token = watch::Receiver<Arc<str>>;

pub struct HttpDispatcher {
    id: String,
    url: String,
    token: Option<Token>,
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
        Self::reloadable(id, url, token.map(|t| watch::channel(Arc::from(t)).1), client)
    }

    #[must_use]
    pub fn reloadable(
        id: impl Into<String>,
        url: impl Into<String>,
        token: Option<Token>,
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
            let current = token.borrow().clone();
            builder = builder.bearer_auth(current);
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
    use std::sync::Arc;

    use axum::http::HeaderMap;
    use cctui_proto::api::{DispatchExposure, HumanGate};

    use super::{DispatchSpec, Dispatcher, HttpDispatcher};

    #[tokio::test]
    async fn a_rotated_token_is_sent_from_the_next_dispatch_on() {
        crate::client::install_crypto_provider();
        let (seen_tx, mut seen) = tokio::sync::mpsc::unbounded_channel::<String>();
        let app = axum::Router::new().route(
            "/dispatch",
            axum::routing::post(move |headers: HeaderMap| {
                let auth = headers["authorization"].to_str().unwrap_or_default().to_owned();
                seen_tx.send(auth).unwrap();
                async { axum::Json(serde_json::json!({ "handle": "job/w" })) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/dispatch", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let (rotate, token) = tokio::sync::watch::channel(Arc::<str>::from("tok-v1"));
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let dispatcher = HttpDispatcher::reloadable("remote", url, Some(token), client);
        let payload = serde_json::json!({});
        let spec = DispatchSpec {
            session_id: "s",
            timeout_minutes: None,
            reply_url: None,
            dedup_key: None,
            payload: &payload,
        };

        dispatcher.dispatch(&spec).await.unwrap();
        assert_eq!(seen.recv().await.unwrap(), "Bearer tok-v1");
        rotate.send_replace(Arc::from("tok-v2"));
        dispatcher.dispatch(&spec).await.unwrap();
        assert_eq!(seen.recv().await.unwrap(), "Bearer tok-v2");
        server.abort();
    }

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
