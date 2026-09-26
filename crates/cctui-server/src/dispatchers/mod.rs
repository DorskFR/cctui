//! Pluggable [`Dispatcher`]s that turn a [`DispatchSpec`] into a launched
//! session: [`enrolled::EnrolledDispatcher`] sends a key-checked dispatch frame
//! to an enrolled executor over its WS (the server needs no kube/docker
//! access), and [`http::HttpDispatcher`] forwards to an env-configured external
//! endpoint as the only in-process fallback.

use async_trait::async_trait;

pub mod enrolled;
pub mod http;

#[derive(Debug, Clone)]
pub struct DispatchHandle {
    pub handle: String,
    pub namespace: Option<String>,
    /// `None` falls back to `"dispatched"`. Also `deduplicated` (an in-flight
    /// Job will call back) and `redispatched` (a terminal Job was recreated).
    pub status: Option<String>,
}

/// Built by the route from a [`cctui_proto::api::DispatchRequest`].
pub struct DispatchSpec<'a> {
    pub session_id: &'a str,
    pub timeout_minutes: Option<u32>,
    /// A bearer capability; do not log.
    pub reply_url: Option<&'a str>,
    /// Hashed into the worker Job name; `None` ⇒ derived from `session_id`.
    pub dedup_key: Option<&'a str>,
    /// Forwarded verbatim to the runtime.
    pub payload: &'a serde_json::Value,
}

#[derive(Debug, thiserror::Error)]
pub enum DispatchError {
    #[error("unknown dispatcher: {0}")]
    UnknownDispatcher(String),
    #[error("intent validation failed: {0}")]
    InvalidIntent(String),
    #[error("backend error: {0}")]
    Backend(String),
    #[error("operation not supported by dispatcher: {0}")]
    Unsupported(String),
}

/// No impl reports a live status yet (the webhook treats `Unsupported` as `Wait`).
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum HandleStatus {
    Running,
    Complete,
    /// Carries the dispatcher's human reason, surfaced in the webhook `error`.
    Failed(Option<String>),
    /// Already GC'd or never created.
    Gone,
}

#[async_trait]
pub trait Dispatcher: Send + Sync {
    /// Matches `DispatchRequest::dispatcher`.
    fn id(&self) -> &str;

    /// The returned handle is opaque and persisted beside the session row.
    async fn dispatch(&self, spec: &DispatchSpec<'_>) -> Result<DispatchHandle, DispatchError>;

    #[allow(dead_code)]
    async fn status(&self, handle: &str) -> Result<HandleStatus, DispatchError> {
        Err(DispatchError::Unsupported(format!("status({handle})")))
    }

    #[allow(dead_code)]
    async fn cancel(&self, handle: &str) -> Result<(), DispatchError> {
        Err(DispatchError::Unsupported(format!("cancel({handle})")))
    }
}

/// Resolves dispatcher id strings to the env-configured in-process impls.
pub struct Registry {
    dispatchers: std::collections::HashMap<String, std::sync::Arc<dyn Dispatcher>>,
}

impl Registry {
    pub fn new() -> Self {
        Self { dispatchers: std::collections::HashMap::new() }
    }

    pub fn with(mut self, d: std::sync::Arc<dyn Dispatcher>) -> Self {
        self.dispatchers.insert(d.id().to_owned(), d);
        self
    }

    pub fn get(&self, id: &str) -> Result<std::sync::Arc<dyn Dispatcher>, DispatchError> {
        self.dispatchers
            .get(id)
            .cloned()
            .ok_or_else(|| DispatchError::UnknownDispatcher(id.to_owned()))
    }

    pub fn ids(&self) -> Vec<String> {
        self.dispatchers.keys().cloned().collect()
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}
