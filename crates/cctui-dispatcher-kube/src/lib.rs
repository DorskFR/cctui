//! `cctui-dispatcher-kube` — a standalone, per-account *enrolled* executor
//! that the cctui-server reaches over the wire.
//!
//! # Model
//!
//! The server is an orchestrator only — it never touches the kube API and
//! carries no k8s RBAC. Dispatchers are standalone executor services deployed
//! per cluster (a plain Deployment with its own ServiceAccount/RBAC) and
//! **enrolled to a user account** (a peer of a "machine"); the verb is Dispatch
//! instead of spawn. On a key-checked `Dispatch` command the dispatcher spawns
//! a worker Job (the image) in its cluster and relays the dispatch info
//! (session id, payload, machine key, server URL) into the pod env. The
//! cctui-daemon *inside* the spawned pod then runs the session. The dispatcher
//! does not track sessions or manage adapters; it spawns pods, period.
//!
//! # Scope
//!
//! This crate is the executor: the enroll CLI, the dial-out WS run loop
//! (heartbeat + half-open recovery, daemon parity), and the kube Job spawn
//! mechanics. It dials the server's `POST /api/v1/dispatcher/{enroll,auth}` +
//! `GET /api/v1/dispatcher/ws` (`crates/cctui-server/src/routes/dispatcher.rs`),
//! which `resolve_dispatcher` relays `Dispatch` commands over
//! (`crates/cctui-server/src/dispatchers/enrolled.rs`), speaking
//! `cctui_proto::ws::Dispatcher*`.
//!
//! ⚠️ Repo is PUBLIC — no homelab namespaces/images/registries baked in;
//! everything environment-specific comes from `dispatcher.toml` / enroll flags.

pub mod config;
pub mod spawn;
