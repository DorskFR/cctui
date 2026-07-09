use anyhow::{Context, Result};
use cctui_proto::api::SessionListResponse;
use cctui_proto::ws::{ServerEvent, TuiCommand};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

pub struct ServerClient {
    base_url: String,
    token: String,
    http: reqwest::Client,
}

impl ServerClient {
    pub fn new(base_url: impl Into<String>, token: impl Into<String>) -> Self {
        Self { base_url: base_url.into(), token: token.into(), http: reqwest::Client::new() }
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

    pub async fn kill_session(&self, session_id: &str) -> Result<()> {
        let url = format!("{}/api/v1/sessions/{}/kill", self.base_url, session_id);
        self.http
            .post(&url)
            .bearer_auth(&self.token)
            .send()
            .await
            .context("POST kill")?
            .error_for_status()
            .context("kill response status")?;
        Ok(())
    }

    /// Interrupt the in-flight turn without tearing the session down (CCT-151).
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

    /// One-call session diagnose (CCT-547): everything the daemon knows about
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

    /// Toggle cctui-side auto-approve for a session (CCT-151).
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

    pub async fn connect_ws(
        &self,
    ) -> Result<(mpsc::Sender<TuiCommand>, mpsc::Receiver<ServerEvent>)> {
        // Authenticate the WS upgrade via the `Authorization` header rather than a
        // `?token=` query param so the token never lands in server access logs
        // (CCT-423). `bearer_or_cookie` on the server accepts either the header
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
        let (event_tx, event_rx) = mpsc::channel::<ServerEvent>(64);

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
                let Ok(event) = serde_json::from_str::<ServerEvent>(&text) else { continue };
                if event_tx.send(event).await.is_err() {
                    break;
                }
            }
        });

        Ok((cmd_tx, event_rx))
    }
}
