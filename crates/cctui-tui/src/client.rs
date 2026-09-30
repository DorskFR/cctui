use anyhow::{Context, Result};
use cctui_proto::api::SessionListResponse;
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Page {
    pub before: Option<i64>,
    pub after: Option<i64>,
    pub limit: Option<i64>,
}

/// `event` keeps the server's object untouched, so deserializing it to an
/// `AgentEvent` still sees `seq`/`ts`/`turn_id`.
#[derive(Debug, Clone, PartialEq)]
pub struct ConversationRow {
    pub seq: i64,
    pub ts: Option<i64>,
    pub turn_id: Option<uuid::Uuid>,
    pub event: Value,
}

pub enum ConversationFetch {
    NotModified,
    Page { rows: Vec<ConversationRow>, etag: Option<String>, has_more: bool },
}

impl ConversationRow {
    fn from_event(event: Value) -> Option<Self> {
        let seq = event.get("seq").and_then(Value::as_i64)?;
        let ts = event.get("ts").and_then(Value::as_i64);
        let turn_id =
            event.get("turn_id").and_then(Value::as_str).and_then(|s| s.parse().ok());
        Some(Self { seq, ts, turn_id, event })
    }
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

    /// One page of a session's transcript. `etag` is replayed as
    /// `If-None-Match`; a 304 comes back as [`ConversationFetch::NotModified`].
    pub async fn conversation(
        &self,
        session_id: &str,
        page: Page,
        etag: Option<&str>,
    ) -> Result<ConversationFetch> {
        let url = format!("{}/api/v1/sessions/{session_id}/conversation", self.base_url);
        let mut request = self.http.get(&url).bearer_auth(&self.token);
        for (key, value) in
            [("before", page.before), ("after", page.after), ("limit", page.limit)]
        {
            if let Some(value) = value {
                request = request.query(&[(key, value)]);
            }
        }
        if let Some(etag) = etag {
            request = request.header(reqwest::header::IF_NONE_MATCH, etag);
        }

        let resp = request.send().await.context("GET conversation")?;
        if resp.status() == reqwest::StatusCode::NOT_MODIFIED {
            return Ok(ConversationFetch::NotModified);
        }
        let resp = resp.error_for_status().context("conversation response status")?;
        // Envoy strips `ETag` off compressed responses; the server mirrors it.
        let etag = resp
            .headers()
            .get(reqwest::header::ETAG)
            .or_else(|| resp.headers().get("x-etag"))
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let events = resp.json::<Vec<Value>>().await.context("deserialize conversation")?;
        let has_more = page.limit.is_some_and(|l| i64::try_from(events.len()).unwrap_or(l) >= l);
        let rows = events.into_iter().filter_map(ConversationRow::from_event).collect();
        Ok(ConversationFetch::Page { rows, etag, has_more })
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
