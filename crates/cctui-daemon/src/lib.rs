//! cctui-daemon: per-machine agent supervisor.
//!
//! Authenticates to a cctui-server with a long-lived machine key, opens a
//! bidirectional WS, instantiates compiled-in adapter modules, and bridges
//! adapter events ↔ server frames.
//!
//! See `crates/cctui-daemon/src/main.rs` for the CLI entry point.

pub mod adapter_runtime;
pub mod adapters;
pub mod askhook;
pub mod blobs;
pub mod bus;
pub mod childenv;
pub mod client;
pub mod config;
pub mod counters;
pub mod dispatch_codex;
pub mod enroll;
pub mod imagepost;
pub mod listdirs;
pub mod runlock;
pub mod runtime;
pub mod selfupdate;
pub mod sendguard;
pub mod service;
pub mod supervisor;
pub mod whipstop;
