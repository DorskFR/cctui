//! Dispatch and machine execution: dispatchers, worker identity, launches,
//! child sessions and their limits, and execution targets.
//!
//! Depends only on the kernel; the host supplies account, session, notify and
//! presence behaviour through [`context`] and [`hooks`].

pub mod context;
pub mod hooks;
pub mod routes;

pub mod child_exec;
pub mod child_policy;
pub mod dispatch;
pub mod dispatch_archive;
pub mod dispatch_identity;
pub mod dispatch_liveness;
pub mod dispatch_policy;
pub mod dispatchers;
pub mod execution_failures;
pub mod launch;
pub mod launch_credentials;
pub mod machine_adapters;
pub mod machine_launches;
pub mod machine_liveness;
pub mod machine_policy;
pub mod machine_resources;
pub mod machine_spawn;
pub mod session_limits;
pub mod spawn_defaults;
pub mod store;
pub mod targets;
pub mod webhook;
pub mod worker_retirement;
