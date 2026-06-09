//! `cctui-dispatcher-docker` — a standalone, per-account *enrolled* executor
//! (CCT-246) that the cctui-server reaches over the wire (CCT-248).
//!
//! # Model (CCT-248)
//!
//! The server is an orchestrator only — it never touches the docker API.
//! Dispatchers are standalone executor services installed per machine and
//! **enrolled to a user account** (a peer of a "machine"); the verb is
//! Dispatch instead of spawn. On a key-checked `Dispatch` command the
//! dispatcher spawns a worker container (the CCT-245 image) on its host and
//! relays the dispatch info (session id, payload, machine key, server URL) into
//! the container env. The cctui-daemon *inside* the spawned container then runs
//! the session. The dispatcher does not track sessions or manage adapters; it
//! spawns containers, period.
//!
//! # Scope / deferral (CCT-246)
//!
//! This crate ships the executor end of the spec end to end: the enroll CLI,
//! the dial-out WS run loop (heartbeat + half-open recovery, daemon parity),
//! and the docker spawn mechanics (lifted from the transitional in-process
//! `cctui-server/src/dispatchers/docker.rs`, which is intentionally **left in
//! place** until CCT-248 parts 2-4 land and soak).
//!
//! The matching **server-side surface it dials** — `POST
//! /api/v1/dispatcher/{enroll,auth}`, `GET /api/v1/dispatcher/ws`, the
//! enrolled-dispatcher table rework, and flipping `resolve_dispatcher` to relay
//! `Dispatch` over this WS — is explicitly gated on this binary (+ the kube
//! one) landing first, per CCT-248's own sequencing (CCT-245 → CCT-246/247 →
//! CCT-248 parts 2-4). Those routes therefore do not exist yet; this binary
//! speaks the decided wire protocol (`cctui_proto::ws::Dispatcher*`) and will
//! connect once that surface lands. Until then it compiles, self-tests, and
//! does not touch the existing working in-process dispatch path.
//!
//! ⚠️ Repo is PUBLIC — no homelab images/hosts/networks baked in; everything
//! environment-specific comes from `dispatcher.toml` / enroll flags.

pub mod client;
pub mod config;
pub mod run;
pub mod spawn;
