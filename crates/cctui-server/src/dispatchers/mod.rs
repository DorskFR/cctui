//! Pluggable [`Dispatcher`]s that turn a [`DispatchSpec`] into a launched
//! session. Impls: [`http::HttpDispatcher`] (forward to an external endpoint),
//! [`kube::KubeDispatcher`] (clone the claude-worker `CronJob` template into a
//! one-shot k8s Job, in-process — retires the external python dispatcher,
//! CCT-234) and [`docker::DockerDispatcher`] (run the worker image as a
//! container via bollard, only when a docker socket is configured).
//!
//! CORRECTED MODEL (CCT-248): the in-process `kube`/`docker` dispatchers below
//! are a *transitional* shape and are slated for removal. The server is an
//! orchestrator only — it must never need kube/docker API access. The intended
//! model is that dispatchers are **standalone executor services enrolled per
//! account as peers of a "machine"**: the server sends a key-checked Dispatch
//! command over the wire (the [`http::HttpDispatcher`] forward-to-endpoint
//! shape is the correct server-side pattern) and the enrolled dispatcher
//! (CCT-246 docker / CCT-247 kubernetes) spawns the worker container/pod. The
//! in-process `kube.rs`/`docker.rs` are deleted once CCT-246/247 land and soak;
//! a plain [`http::HttpDispatcher`] stays as the escape hatch for fully
//! external systems. See CCT-248 for the enrollment + dial-out-WS transport
//! spec the rest of the epic builds on.
//!
//! Lifecycle: the route ([`crate::routes::dispatch`]) mints a session id,
//! inserts a row in `sessions` (`origin = '<dispatcher_id>'`, status
//! `new`), then calls [`Dispatcher::dispatch`]. The worker pod is
//! responsible for taking that pre-minted id back to `/sessions/register`
//! once cctui-daemon lands inside the image (separate ticket). Until
//! then, dispatched sessions stay in `new` and remain visible in the
//! sessions table as "in flight" via their `origin` + `dispatch_handle`.

use async_trait::async_trait;

pub mod docker;
pub mod http;
pub mod kube;
pub mod stored;

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
    /// The dispatcher does not implement this operation (e.g. `HttpDispatcher`
    /// can dispatch but not introspect/cancel a handle it forwarded).
    #[error("operation not supported by dispatcher: {0}")]
    Unsupported(String),
}

/// Lifecycle state of a dispatched handle, reported by [`Dispatcher::status`].
///
/// Part of the trait surface added in CCT-234 (`status`/`cancel`); consumed by
/// the kube/docker dispatchers and reserved for a future observe/cancel route,
/// so the variants are allowed to be unused for now.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum HandleStatus {
    /// The Job/container is still pending or running.
    Running,
    /// The Job/container finished successfully.
    Complete,
    /// The Job/container failed (backoff exhausted, deadline, non-zero exit).
    Failed,
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
}

/// Resolves dispatcher id strings to concrete impls. Built once at
/// startup and shared through `AppState`.
///
/// CCT-248: in the corrected model, dispatch resolution targets an
/// *enrolled* dispatcher (a per-account peer of a machine) and dispatches by
/// sending a key-checked command over that dispatcher's live connection,
/// rather than instantiating an in-process `Arc<dyn Dispatcher>` that talks to
/// a runtime API directly. This in-process registry remains only for the env-
/// configured plain-`http` escape hatch and the transitional `kube`/`docker`
/// impls until CCT-246/247 land.
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
