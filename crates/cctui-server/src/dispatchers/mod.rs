//! Pluggable [`Dispatcher`]s that turn a [`DispatchSpec`] into a launched
//! session. Impls:
//!   * [`enrolled::EnrolledDispatcher`] — the primary model: a standalone
//!     executor service (cctui-dispatcher-kube / -docker) that enrolled per
//!     account and dials out over `/api/v1/dispatcher/ws`. The server sends a
//!     key-checked [`cctui_proto::ws::DispatcherFrameDown::Dispatch`] over the
//!     hub and awaits the [`cctui_proto::ws::DispatcherFrameUp::DispatchResult`]
//!     reply. The server never needs kube/docker API access.
//!   * [`http::HttpDispatcher`] — the escape hatch: forward a dispatch to a
//!     fully external HTTP endpoint (env-configured global registry only).
//!
//! CCT-292: the transitional in-process `kube`/`docker` dispatchers (CCT-234,
//! restored in CCT-360) are removed now that prod dispatches exclusively
//! through the enrolled executor binaries. Dispatch resolution is enrolled-first
//! with the `http` escape hatch as the only in-process fallback.

use async_trait::async_trait;

pub mod enrolled;
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
    /// Idempotency / dedup key (CCT-522): the caller's logical request id, hashed
    /// by the dispatcher into the worker Job name so a fresh-per-dispatch
    /// `session_id` no longer chains conversations. `None` ⇒ derive from
    /// `session_id` (each dispatch unique).
    pub dedup_key: Option<&'a str>,
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
    /// The dispatcher does not implement this operation (e.g. `HttpDispatcher`
    /// can dispatch but not introspect/cancel a handle it forwarded).
    #[error("operation not supported by dispatcher: {0}")]
    Unsupported(String),
}

/// Lifecycle state of a dispatched handle, reported by [`Dispatcher::status`].
///
/// Part of the trait surface added in CCT-234 (`status`/`cancel`). With the
/// in-process kube/docker dispatchers gone (CCT-292) no impl reports a live
/// status today — the enrolled/http dispatchers return `Unsupported` and the
/// completion webhook treats that as `Wait` — so the variants are reserved for
/// a future observe/cancel route and allowed to be unused for now.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum HandleStatus {
    /// The Job/container is still pending or running.
    Running,
    /// The Job/container finished successfully.
    Complete,
    /// The Job/container failed (backoff exhausted, deadline, non-zero exit,
    /// `CrashLoopBackOff`, `OOMKilled`, unschedulable). Carries the dispatcher's
    /// human reason when it has one, surfaced in the completion webhook's
    /// `error` field (CCT-429).
    Failed(Option<String>),
    /// No Job/container with this handle exists (already GC'd or never created).
    Gone,
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

    /// Inspect a previously returned handle. Defaults to `Unsupported` so
    /// `HttpDispatcher` (which forwards to an opaque endpoint) need not
    /// implement it; the native kube/docker dispatchers override it.
    #[allow(dead_code)]
    async fn status(&self, handle: &str) -> Result<HandleStatus, DispatchError> {
        Err(DispatchError::Unsupported(format!("status({handle})")))
    }

    /// Cancel/delete a previously returned handle. Defaults to `Unsupported`.
    #[allow(dead_code)]
    async fn cancel(&self, handle: &str) -> Result<(), DispatchError> {
        Err(DispatchError::Unsupported(format!("cancel({handle})")))
    }

    /// The IP of a live PEER server replica holding this dispatcher's WS, when
    /// this pod doesn't (CCT-567). The dispatch route checks it BEFORE any side
    /// effects (ntfy, key minting) and answers 421 so [`crate::forward`]
    /// re-sends the request to the owning pod. `None` — the default, and always
    /// for `HttpDispatcher` (no WS involved) — means "handle it here".
    async fn remote_owner(&self) -> Option<String> {
        None
    }
}

/// Resolves dispatcher id strings to concrete impls. Built once at
/// startup and shared through `AppState`.
///
/// CCT-285: dispatch resolution targets an *enrolled* dispatcher (a per-account
/// peer of a machine) and dispatches by sending a key-checked command over that
/// dispatcher's live WS connection ([`enrolled::EnrolledDispatcher`]). This
/// in-process registry now holds only the env-configured plain-`http` escape
/// hatch ([`http::HttpDispatcher`]).
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
