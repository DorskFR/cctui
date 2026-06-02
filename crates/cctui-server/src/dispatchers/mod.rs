//! Pluggable [`Dispatcher`]s that turn a [`DispatchSpec`] into a launched
//! session. Only impl is [`http::HttpDispatcher`].
//!
//! Lifecycle: the route ([`crate::routes::dispatch`]) mints a session id,
//! inserts a row in `sessions` (`origin = '<dispatcher_id>'`, status
//! `new`), then calls [`Dispatcher::dispatch`]. The worker pod is
//! responsible for taking that pre-minted id back to `/sessions/register`
//! once cctui-daemon lands inside the image (separate ticket). Until
//! then, dispatched sessions stay in `new` and remain visible in the
//! sessions table as "in flight" via their `origin` + `dispatch_handle`.

use async_trait::async_trait;

pub mod http;

#[derive(Debug, Clone)]
pub struct DispatchHandle {
    pub handle: String,
    pub namespace: Option<String>,
    /// Outcome reported by the dispatcher, surfaced to the caller verbatim
    /// (CCT-207). `None` when the dispatcher predates the field; the route then
    /// falls back to `"dispatched"`. Known values: `dispatched` (fresh run),
    /// `deduplicated` (in-flight Job — the original run still calls back),
    /// `redispatched` (a terminal Job was deleted + recreated so a fresh run
    /// calls back, instead of the caller parking on a dead Job).
    pub status: Option<String>,
}

/// Everything a [`Dispatcher`] needs to materialize a session. Built by the
/// route from a [`cctui_proto::api::DispatchRequest`]. `payload` is opaque —
/// dispatchers forward it to their runtime without inspecting it.
pub struct DispatchSpec<'a> {
    /// Pre-minted session id (also the runtime's correlation id).
    pub session_id: &'a str,
    /// Per-flow timeout in minutes, if the caller set one.
    pub timeout_minutes: Option<u32>,
    /// Caller resume URL — a bearer capability; do not log.
    pub reply_url: Option<&'a str>,
    /// Free-form blob, forwarded verbatim to the runtime.
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
}

#[async_trait]
pub trait Dispatcher: Send + Sync {
    /// Stable identifier matching `DispatchRequest::dispatcher`. Borrowed
    /// (not `&'static`) so config-driven dispatchers can own their id.
    fn id(&self) -> &str;

    /// Materialize the request for the pre-minted `session_id`. The
    /// returned handle is opaque per-dispatcher (e.g. `"jobs/foo-…"`)
    /// and persisted alongside the session row for observability.
    async fn dispatch(&self, spec: &DispatchSpec<'_>) -> Result<DispatchHandle, DispatchError>;
}

/// Resolves dispatcher id strings to concrete impls. Built once at
/// startup and shared through `AppState`.
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
