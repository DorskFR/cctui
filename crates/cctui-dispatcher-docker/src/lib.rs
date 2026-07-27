//! `cctui-dispatcher-docker` — a standalone, per-account *enrolled* executor
//! that the cctui-server reaches over the wire.
//!
//! # Model
//!
//! The server is an orchestrator only — it never touches the docker API.
//! Dispatchers are standalone executor services installed per machine and
//! **enrolled to a user account** (a peer of a "machine"); the verb is
//! Dispatch instead of spawn. On a key-checked `Dispatch` command the
//! dispatcher spawns a worker container (the image) on its host and
//! relays the dispatch info (session id, payload, machine key, server URL) into
//! the container env. The cctui-daemon *inside* the spawned container then runs
//! the session. The dispatcher does not track sessions or manage adapters; it
//! spawns containers, period.
//!
//! # Scope
//!
//! This crate is the executor: the enroll CLI, the dial-out WS run loop
//! (heartbeat + half-open recovery, daemon parity), and the docker spawn
//! mechanics. It dials the server's `POST /api/v1/dispatcher/{enroll,auth}` +
//! `GET /api/v1/dispatcher/ws` (`crates/cctui-server/src/routes/dispatcher.rs`),
//! which `resolve_dispatcher` relays `Dispatch` commands over
//! (`crates/cctui-server/src/dispatchers/enrolled.rs`), speaking
//! `cctui_proto::ws::Dispatcher*`.
//!
//! ⚠️ Repo is PUBLIC — no homelab images/hosts/networks baked in; everything
//! environment-specific comes from `dispatcher.toml` / enroll flags.

pub mod client;
pub mod config;
pub mod run;
pub mod spawn;
