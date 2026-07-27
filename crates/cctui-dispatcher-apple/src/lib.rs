//! `cctui-dispatcher-apple` — a standalone, per-account *enrolled* executor
//! that spawns the cctui-worker image via Apple's `container` CLI
//! (Virtualization.framework, one micro-VM per container). Peer of the docker
//! and kube dispatchers; the server reaches it over the wire.
//!
//! # Model
//!
//! The server is an orchestrator only — it never touches a container runtime.
//! Dispatchers are standalone executor services installed per machine and
//! **enrolled to a user account** (a peer of a "machine"); the verb is Dispatch
//! instead of spawn. On a key-checked `Dispatch` command the dispatcher runs
//! `container run <worker-image>` on its host and relays the dispatch info
//! (session id, payload, machine key, server URL) to the cctui-daemon *inside*
//! the spawned micro-VM. The dispatcher does not track sessions or manage
//! adapters; it spawns containers, period.
//!
//! # Apple `container` specifics
//!
//! Apple's `container` has **no clone/snapshot/commit** (confirmed against the
//! June 2026 CLI: create/run/ls/inspect/set/logs/stop/delete only). The model is
//! therefore image-based — boot a fresh micro-VM from an OCI image, never
//! clone-a-live-VM — with **no APFS `CoW`** equivalent. Every session gets its own
//! IP and network namespace, the isolation a git worktree cannot offer.
//!
//! # Boot contract (shared with 247)
//!
//! OCI image + secret + optional repo mount. The machine key is delivered as a
//! **mounted file** by default rather than an env var — a token in `container
//! inspect` / the guest process list is visible. The guest reads it
//! from `CCTUI_MACHINE_KEY_FILE`. Identity/credentials are injected at spawn,
//! never baked into the image. The repo layer is a `git pull --depth 1` (shallow)
//! at boot — fresh-checkout semantics, signalled to the guest via an optional
//! repo mount + `CCTUI_GIT_SHALLOW`.
//!
//! # Testability
//!
//! The `container` binary only exists on macOS, so all spawn mechanics go through
//! the [`cli::ContainerCli`] trait: a real [`cli::RealCli`] shells out to
//! `container run|inspect|stop|delete`; a mock drives the lifecycle in unit tests
//! on any host. Argument construction is pure and tested directly.
//!
//! ⚠️ Repo is PUBLIC — no homelab images/hosts/networks baked in; everything
//! environment-specific comes from `dispatcher.toml` / enroll flags.

pub mod cli;
pub mod client;
pub mod config;
pub mod run;
pub mod spawn;
