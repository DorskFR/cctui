//! `cctui-dispatcher-kube` — a standalone, per-account *enrolled* executor
//! that the cctui-server reaches over the wire.
//!
//! The server never touches the kube API. On a key-checked `Dispatch` command this
//! executor spawns a worker Job in its cluster and passes the session identity to the
//! cctui-daemon inside; it does not track sessions or manage adapters.
//!
//! The crate holds the enroll CLI, the dial-out WS run loop against
//! `/api/v1/dispatcher/{enroll,auth,ws}` (`cctui_proto::ws::Dispatcher*`), and
//! the spawn mechanics.
//!
//! ⚠️ Repo is PUBLIC — nothing environment-specific baked in; it comes from
//! `dispatcher.toml` / enroll flags.

pub mod backend;
pub mod config;
pub mod spawn;
