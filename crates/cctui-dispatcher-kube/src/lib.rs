//! `cctui-dispatcher-kube` — a standalone, per-account *enrolled* executor
//! (CCT-247) that the cctui-server reaches over the wire (CCT-248).
//!
//! # Model (CCT-248)
//!
//! The server is an orchestrator only — it never touches the kube API and
//! carries no k8s RBAC. Dispatchers are standalone executor services deployed
//! per cluster (a plain Deployment with its own ServiceAccount/RBAC) and
//! **enrolled to a user account** (a peer of a "machine"); the verb is Dispatch
//! instead of spawn. On a key-checked `Dispatch` command the dispatcher spawns
//! a worker Job (the CCT-245 image) in its cluster and relays the dispatch info
//! (session id, payload, machine key, server URL) into the pod env. The
//! cctui-daemon *inside* the spawned pod then runs the session. The dispatcher
//! does not track sessions or manage adapters; it spawns pods, period.
//!
//! # Scope / deferral (CCT-247)
//!
//! This crate ships the executor end of the spec end to end: the enroll CLI,
//! the dial-out WS run loop (heartbeat + half-open recovery, daemon parity),
//! and the kube Job spawn mechanics (lifted from the transitional in-process
//! `cctui-server/src/dispatchers/kube.rs`, which is intentionally **left in
//! place** until CCT-248 parts 2-4 land and soak).
//!
//! The matching **server-side surface it dials** — `POST
//! /api/v1/dispatcher/{enroll,auth}`, `GET /api/v1/dispatcher/ws`, the
//! enrolled-dispatcher table rework, and flipping `resolve_dispatcher` to relay
//! `Dispatch` over this WS — is explicitly gated on this binary (+ the docker
//! one) landing first, per CCT-248's own sequencing (CCT-245 → CCT-246/247 →
//! CCT-248 parts 2-4). Those routes therefore do not exist yet; this binary
//! speaks the decided wire protocol (`cctui_proto::ws::Dispatcher*`) and will
//! connect once that surface lands. Until then it compiles, self-tests, and
//! does not touch the existing working in-process dispatch path.
//!
//! ⚠️ Repo is PUBLIC — no homelab namespaces/images/registries baked in;
//! everything environment-specific comes from `dispatcher.toml` / enroll flags.

pub mod client;
pub mod config;
pub mod run;
pub mod spawn;
