use anyhow::{Context, Result};
use cctui_proto::api::SessionListResponse;
use cctui_proto::api::me::MeResponse;
use cctui_proto::ws::{ServerEvent, TuiCommand};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

/// A frame off the websocket. Anything that fails to deserialize arrives as
/// [`Incoming::Undecodable`] rather than vanishing.
pub enum Incoming {
    Event(Box<ServerEvent>),
    Undecodable(String),
}

/// The single place a websocket frame becomes an [`Incoming`]. Shared with the
/// contract test so it exercises the production decode, not a copy of it.
pub fn decode_frame(text: &str) -> Incoming {
    match serde_json::from_str::<ServerEvent>(text) {
        Ok(event) => Incoming::Event(Box::new(event)),
        Err(e) => Incoming::Undecodable(e.to_string()),
    }
}

/// A failed request, with the rejected credential told apart from everything
/// else: only a 401 means the key is the problem.
#[derive(Debug)]
pub enum ApiError {
    Unauthorized,
    Other(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unauthorized => f.write_str("unauthorized"),
            Self::Other(e) => f.write_str(e),
        }
    }
}

/// Whether an `anyhow` error from one of the untyped calls is really a 401, so
/// a rejected key surfaces as such instead of as a generic failure.
#[must_use]
pub fn is_unauthorized(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<reqwest::Error>()
            .is_some_and(|e| e.status() == Some(reqwest::StatusCode::UNAUTHORIZED))
    })
}

pub struct ServerClient {
    base_url: String,
    token: String,
    http: reqwest::Client,
}

impl ServerClient {
    pub fn new(base_url: impl Into<String>, token: impl Into<String>) -> Self {
        let _ = rustls::crypto::ring::default_provider().install_default();
        Self { base_url: base_url.into(), token: token.into(), http: reqwest::Client::new() }
    }

    /// The identity behind the configured key. Routed through the shared route
    /// table so the path is never spelled here.
    pub async fn me(&self) -> std::result::Result<MeResponse, ApiError> {
        let route = cctui_proto::api::routes::by_id("get_me").ok_or_else(|| {
            ApiError::Other("route table has no get_me entry".to_owned())
        })?;
        let url = format!("{}{}", self.base_url, route.url(&[]));
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|e| ApiError::Other(e.to_string()))?;
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(ApiError::Unauthorized);
        }
        resp.error_for_status()
            .map_err(|e| ApiError::Other(e.to_string()))?
            .json::<MeResponse>()
            .await
            .map_err(|e| ApiError::Other(e.to_string()))
    }

    /// Revoke the key this client authenticates with (`cctui logout --revoke`).
    pub async fn revoke_current_key(&self) -> std::result::Result<(), ApiError> {
        let route = cctui_proto::api::routes::by_id("delete_me_key")
            .ok_or_else(|| ApiError::Other("route table has no delete_me_key entry".to_owned()))?;
        let url = format!("{}{}", self.base_url, route.url(&[]));
        let resp = self
            .http
            .delete(&url)
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|e| ApiError::Other(e.to_string()))?;
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(ApiError::Unauthorized);
        }
        resp.error_for_status().map_err(|e| ApiError::Other(e.to_string()))?;
        Ok(())
    }

    pub async fn list_sessions(&self) -> Result<SessionListResponse> {
        let url = format!("{}/api/v1/sessions", self.base_url);
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.token)
            .send()
            .await
            .context("GET /api/v1/sessions")?
            .error_for_status()
            .context("sessions response status")?
            .json::<SessionListResponse>()
            .await
            .context("deserialize sessions")?;
        Ok(resp)
    }

    pub async fn get_conversation(&self, session_id: &str) -> Result<Vec<Value>> {
        let url = format!("{}/api/v1/sessions/{}/conversation", self.base_url, session_id);
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.token)
            .send()
            .await
            .context("GET conversation")?
            .error_for_status()
            .context("conversation response status")?
            .json::<Vec<Value>>()
            .await
            .context("deserialize conversation")?;
        Ok(resp)
    }

    /// Interrupt the in-flight turn without tearing the session down.
    pub async fn interrupt_session(&self, session_id: &str) -> Result<()> {
        let url = format!("{}/api/v1/sessions/{}/interrupt", self.base_url, session_id);
        self.http
            .post(&url)
            .bearer_auth(&self.token)
            .send()
            .await
            .context("POST interrupt")?
            .error_for_status()
            .context("interrupt response status")?;
        Ok(())
    }

    /// One-call session diagnose: everything the daemon knows about
    /// the session, dated, plus the server-side binding facts.
    pub async fn diagnose_session(
        &self,
        session_id: &str,
    ) -> Result<cctui_proto::diagnose::SessionDiagnoseResponse> {
        let url = format!("{}/api/v1/sessions/{}/diagnose", self.base_url, session_id);
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.token)
            .send()
            .await
            .context("GET diagnose")?
            .error_for_status()
            .context("diagnose response status")?
            .json::<cctui_proto::diagnose::SessionDiagnoseResponse>()
            .await
            .context("deserialize diagnose")?;
        Ok(resp)
    }

    /// Toggle cctui-side auto-approve for a session.
    pub async fn set_auto_approve(&self, session_id: &str, enabled: bool) -> Result<()> {
        let url = format!("{}/api/v1/sessions/{}/auto-approve", self.base_url, session_id);
        self.http
            .post(&url)
            .bearer_auth(&self.token)
            .json(&cctui_proto::api::AutoApproveRequest { enabled })
            .send()
            .await
            .context("POST auto-approve")?
            .error_for_status()
            .context("auto-approve response status")?;
        Ok(())
    }

    pub async fn connect_ws(&self) -> Result<(mpsc::Sender<TuiCommand>, mpsc::Receiver<Incoming>)> {
        // Authenticate the WS upgrade via the `Authorization` header rather than a
        // `?token=` query param so the token never lands in server access logs.
        // `bearer_or_cookie` on the server accepts either the header
        // (native clients like this TUI) or the `HttpOnly` cookie (browsers).
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;

        let ws_url = format!(
            "{}/api/v1/ws",
            self.base_url.replacen("http://", "ws://", 1).replacen("https://", "wss://", 1),
        );
        let mut request = ws_url.into_client_request().context("build ws request")?;
        request.headers_mut().insert(
            AUTHORIZATION,
            format!("Bearer {}", self.token).parse().context("build auth header")?,
        );

        let (ws_stream, _) = connect_async(request).await.context("connect websocket")?;
        let (mut ws_sink, mut ws_source) = ws_stream.split();

        let (cmd_tx, mut cmd_rx) = mpsc::channel::<TuiCommand>(64);
        let (event_tx, event_rx) = mpsc::channel::<Incoming>(64);

        // Sender task: forward TuiCommands to WS
        tokio::spawn(async move {
            while let Some(cmd) = cmd_rx.recv().await {
                let Ok(json) = serde_json::to_string(&cmd) else { break };
                if ws_sink.send(Message::Text(json.into())).await.is_err() {
                    break;
                }
            }
        });

        // Receiver task: forward WS messages to ServerEvent channel
        tokio::spawn(async move {
            while let Some(Ok(msg)) = ws_source.next().await {
                let text = match msg {
                    Message::Text(t) => t,
                    Message::Close(_) => break,
                    _ => continue,
                };
                if event_tx.send(decode_frame(&text)).await.is_err() {
                    break;
                }
            }
        });

        Ok((cmd_tx, event_rx))
    }
}
