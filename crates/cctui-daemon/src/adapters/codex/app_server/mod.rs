//! Codex `app-server` driver.
//!
//! Drives sessions that cctui *spawns*, as opposed to the log-tail
//! ([`super::log_tail`]) which passively observes sessions started outside
//! cctui (e.g. the Codex TUI). The two coexist: session identity is the
//! rollout id (`UUIDv7`), so an app-server-driven session and its on-disk
//! rollout file refer to the same `local_id`.
//!
//! `codex app-server` speaks newline-delimited JSON-RPC 2.0 over stdio
//! (stderr is logs). The handshake is `initialize` (declaring client
//! capabilities) → `initialized` notification → `thread/start { cwd }`
//! → `turn/start { threadId, input }`. The minimum supported Codex
//! version and the retained JSON Schema live in [`super::contract`]. A stale
//! cctui-owned thread is revived
//! with `thread/resume { threadId }` before the next `turn/start`.
//! Streaming arrives as id-less
//! notifications (`item/completed`, `turn/completed`, …); tool approvals
//! arrive as server→client *requests* (they carry both `method` and `id`)
//! that block until we reply with a `decision`.
//!
//!
//! The pure protocol layer — the JSON-RPC codec ([`rpc`]), notification
//! mapping ([`notifications`]), request builders ([`requests`]) and turn/item
//! state ([`thread_state`]) — is unit-tested with fixtures; [`CodexSession`]
//! owns the subprocess and pumps IO ([`event_loop`]).

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
