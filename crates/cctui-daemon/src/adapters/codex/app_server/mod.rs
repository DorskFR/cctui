//! Codex `app-server` driver for sessions cctui spawns; [`super::log_tail`]
//! observes the rest. Both key sessions by the rollout `UUIDv7`.
//!
//! JSON-RPC 2.0 over stdio: `initialize` → `initialized` → `thread/start` →
//! `turn/start`; a stale thread is revived with `thread/resume` first.
//! Streaming arrives as id-less notifications; tool approvals are
//! server→client requests that block until answered with a `decision`. The
//! supported version and schema live in [`super::contract`].
//!
//! [`rpc`], [`notifications`], [`requests`] and [`thread_state`] are the pure
//! protocol layer; [`CodexSession`] owns the subprocess and [`event_loop`]
//! pumps its IO.

mod config;
mod diagnose;
mod event_loop;
mod lifecycle;
mod notifications;
mod registry;
mod requests;
mod rpc;
mod session;
mod thread_state;

pub use config::AppServerConfig;
#[cfg(test)]
pub(super) use config::gateway_provider_overrides;
pub use diagnose::{DiagnoseRings, set_ring_scrub, shared_rings};
pub use lifecycle::{LifecycleOp, run_thread_lifecycle};
pub use notifications::item_event;
pub(super) use notifications::{TurnStatus, parse_status};
pub use registry::{
    CodexLiveSnapshot, LiveSessionRegistry, RouteAction, SessionCommand, SessionRecord,
    SessionRegistry, route_or_prepare_resume,
};
pub use requests::{ThreadConfig, normalize_service_tier, service_tier_from_settings};
pub(super) use requests::{initialized_notification, record_codex_version};
pub use session::{CodexSession, spawn_resumed_session};
