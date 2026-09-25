//! `cctui-dispatcher-apple` — a standalone, per-account *enrolled* executor
//! that spawns the cctui-worker image via Apple's `container` CLI (one micro-VM
//! per container). On a key-checked `Dispatch` it runs `container run` and hands
//! the session identity to the cctui-daemon inside; it does not track sessions.
//!
//! `container` has no clone/snapshot, so every session boots fresh from an OCI
//! image with a shallow checkout. The machine key is delivered as a mounted file
//! (`CCTUI_MACHINE_KEY_FILE`), never env, so it stays out of `container inspect`.
//! All spawn mechanics go through [`cli::ContainerCli`] so they are testable off
//! macOS.
//!
//! ⚠️ Repo is PUBLIC — nothing environment-specific baked in; it comes from
//! `dispatcher.toml` / enroll flags.

pub mod backend;
pub mod cli;
pub mod config;
pub mod spawn;
