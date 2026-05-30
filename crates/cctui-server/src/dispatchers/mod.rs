//! Pluggable [`Dispatcher`]s: server-mediated entry points that turn a
//! runtime-agnostic [`DispatchSpec`] into a concrete launched session.
//!
//! Today's only impl is [`k8s_job::K8sJobDispatcher`] which materializes a
//! one-shot `batch/v1` Job from a worker `CronJob` in a configurable
//! namespace. Future impls (codex-cloud, ssh-remote, daemon-spawn
//! wrapping the existing `/sessions/spawn` flow) plug in through the same
//! trait without route changes.
//!
//! Lifecycle: the route ([`crate::routes::dispatch`]) mints a session id,
//! inserts a row in `sessions` (`origin = '<dispatcher_id>'`, status
//! `new`), then calls [`Dispatcher::dispatch`]. The worker pod is
//! responsible for taking that pre-minted id back to `/sessions/register`
//! once cctui-daemon lands inside the image (separate ticket). Until
//! then, dispatched sessions stay in `new` and remain visible in the
//! sessions table as "in flight" via their `origin` + `dispatch_handle`.

use async_trait::async_trait;

pub mod k8s_job;

#[derive(Debug, Clone)]
pub struct DispatchHandle {
    pub handle: String,
    pub namespace: Option<String>,
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
    /// Stable identifier matching `DispatchRequest::dispatcher`.
    fn id(&self) -> &'static str;

    /// Materialize the request for the pre-minted `session_id`. The
    /// returned handle is opaque per-dispatcher (e.g. `"jobs/foo-…"`)
    /// and persisted alongside the session row for observability.
    async fn dispatch(&self, spec: &DispatchSpec<'_>) -> Result<DispatchHandle, DispatchError>;
}

/// Resolves dispatcher id strings to concrete impls. Built once at
/// startup and shared through `AppState`.
pub struct Registry {
    dispatchers: std::collections::HashMap<&'static str, std::sync::Arc<dyn Dispatcher>>,
}

impl Registry {
    pub fn new() -> Self {
        Self { dispatchers: std::collections::HashMap::new() }
    }

    pub fn with(mut self, d: std::sync::Arc<dyn Dispatcher>) -> Self {
        self.dispatchers.insert(d.id(), d);
        self
    }

    pub fn get(&self, id: &str) -> Result<std::sync::Arc<dyn Dispatcher>, DispatchError> {
        self.dispatchers
            .get(id)
            .cloned()
            .ok_or_else(|| DispatchError::UnknownDispatcher(id.to_owned()))
    }

    pub fn ids(&self) -> Vec<&'static str> {
        self.dispatchers.keys().copied().collect()
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}
