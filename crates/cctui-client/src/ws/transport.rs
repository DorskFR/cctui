use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;

use crate::error::ClientError;

/// What one read off a socket yields. Non-text frames still count as traffic
/// for the keepalive watchdog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    Text(String),
    Other,
    Closed,
}

/// One live socket. Split out so the reconnect loop can be driven by a fake.
#[async_trait]
pub trait Transport: Send {
    async fn send_text(&mut self, text: String) -> Result<(), ClientError>;
    async fn send_ping(&mut self) -> Result<(), ClientError>;
    async fn recv(&mut self) -> Option<Result<Frame, ClientError>>;
}

/// Dials a fresh socket. The reconnect loop owns one for the client's lifetime.
#[async_trait]
pub trait Connector: Send + Sync + 'static {
    async fn connect(&self) -> Result<Box<dyn Transport>, ClientError>;
}

pub struct StreamTransport<S> {
    inner: WebSocketStream<S>,
}

impl<S> StreamTransport<S> {
    #[must_use]
    pub const fn new(inner: WebSocketStream<S>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl<S: AsyncRead + AsyncWrite + Unpin + Send> Transport for StreamTransport<S> {
    async fn send_text(&mut self, text: String) -> Result<(), ClientError> {
        self.inner
            .send(Message::Text(text.into()))
            .await
            .map_err(|e| ClientError::Websocket(e.to_string()))
    }

    async fn send_ping(&mut self) -> Result<(), ClientError> {
        self.inner
            .send(Message::Ping(Vec::new().into()))
            .await
            .map_err(|e| ClientError::Websocket(e.to_string()))
    }

    async fn recv(&mut self) -> Option<Result<Frame, ClientError>> {
        match self.inner.next().await? {
            Ok(Message::Text(text)) => Some(Ok(Frame::Text(text.to_string()))),
            Ok(Message::Close(_)) => Some(Ok(Frame::Closed)),
            Ok(_) => Some(Ok(Frame::Other)),
            Err(e) => Some(Err(ClientError::Websocket(e.to_string()))),
        }
    }
}

/// Authenticates the upgrade with an `Authorization` header rather than a
/// `?token=` query param, so the token never lands in access logs.
pub struct HttpConnector {
    url: String,
    token: String,
}

impl HttpConnector {
    #[must_use]
    pub fn new(url: impl Into<String>, token: impl Into<String>) -> Self {
        Self { url: url.into(), token: token.into() }
    }
}

#[async_trait]
impl Connector for HttpConnector {
    async fn connect(&self) -> Result<Box<dyn Transport>, ClientError> {
        let mut request = self
            .url
            .as_str()
            .into_client_request()
            .map_err(|e| ClientError::Websocket(e.to_string()))?;
        let header = format!("Bearer {}", self.token)
            .parse()
            .map_err(|_| ClientError::Websocket("token is not a valid header value".to_owned()))?;
        request.headers_mut().insert(AUTHORIZATION, header);
        let (stream, _) = tokio_tungstenite::connect_async(request)
            .await
            .map_err(|e| ClientError::Websocket(e.to_string()))?;
        Ok(Box::new(StreamTransport::new(stream)))
    }
}
