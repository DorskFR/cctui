/// Everything a client call can fail with.
///
/// `Unauthorized` is separate so callers can show "key rejected — run
/// `cctui login`" rather than a generic HTTP failure.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("unauthorized — the server rejected this key")]
    Unauthorized,
    #[error("forbidden: {route}")]
    Forbidden { route: &'static str },
    #[error("not found: {route}")]
    NotFound { route: &'static str },
    #[error("{route} returned HTTP {status}: {body}")]
    Status { route: &'static str, status: u16, body: String },
    #[error("{route}: {source}")]
    Transport {
        route: &'static str,
        #[source]
        source: reqwest::Error,
    },
    #[error("{route}: malformed response: {source}")]
    Decode {
        route: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("no route with id `{0}` in the route table")]
    UnknownRoute(String),
    #[error("websocket: {0}")]
    Websocket(String),
    #[error("the websocket is not connected")]
    Disconnected,
    #[error("no ack for message `{0}` within the timeout")]
    AckTimeout(String),
}

impl ClientError {
    /// Whether the credential, not the request, is the problem.
    #[must_use]
    pub const fn is_unauthorized(&self) -> bool {
        matches!(self, Self::Unauthorized)
    }
}
