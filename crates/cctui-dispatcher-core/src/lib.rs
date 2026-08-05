//! Shared plumbing for the standalone, per-account *enrolled* dispatchers.
//!
//! An enrolled dispatcher is a peer of a "machine": the server dials nothing —
//! the dispatcher dials out, authenticates, and serves `Dispatch`/`Status`/
//! `Cancel` frames by spawning workers on its platform. This crate owns the
//! wire-facing plumbing shared by every platform: the [`ServerClient`], the
//! [`Runner`] WS loop, and the [`Dispatcher`] trait each platform crate
//! implements with its own workload builder (docker `HostConfig`, kube
//! `PodSpec`, apple plist).

pub mod client;
pub mod dispatcher;
pub mod run;

pub use client::{EnrollResponse, ServerClient};
pub use dispatcher::{
    BaseEnv, Dispatcher, HandleState, SpawnOutcome, build_env, dedup_source, label_safe,
    worker_name,
};
pub use run::Runner;
