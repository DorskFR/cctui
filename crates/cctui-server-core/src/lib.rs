//! Shared server services for automation and the interactive workspace.
//!
//! This crate must never depend on the full server or workspace services.
//! Owns identity, execution persistence, account selection, launch tracking and
//! provider credentials. Hosts supply transport and optional workspace behavior.

pub mod account_pick;
pub mod account_resolve;
pub use cctui_server_kernel::auth;
pub mod commands;
pub mod cost;
pub mod fireworks_billing;
pub mod gateway;
pub mod store;
pub mod usage_probe;

#[cfg(any(test, feature = "test-support"))]
pub use cctui_server_kernel::test_support;

pub use cctui_server_kernel::bus;
pub mod dispatchers;
pub mod registry;

pub use cctui_server_kernel::{client, outbound};

pub mod hook_policy;
pub mod launch;

pub mod launch_settings;

pub mod events;
pub mod lifecycle;
mod live_sessions;

pub mod dispatch;
pub mod dispatch_identity;
pub mod dispatch_policy;

pub mod provider_status;

pub use cctui_server_kernel::error;
pub mod machine_spawn;

pub mod execution_settings;

pub mod session_credentials;

pub mod state;

pub mod identity;
pub mod targets;

pub use cctui_server_kernel::normalize;

pub mod results;

pub mod launch_credentials;

pub mod spawn_defaults;

pub mod accounts;
pub use cctui_server_kernel::resource_owners;
pub mod settings_catalog;

pub mod machine_adapters;
pub mod machine_policy;

pub mod daemon_checkpoints;
pub mod daemon_usage;

pub mod webhook;

pub mod dispatch_liveness;
pub mod schema;
pub mod worker_retirement;

pub mod machine_launches;

pub mod child_policy;

pub mod child_exec;

pub mod session_limits;

pub use cctui_server_kernel::envelope_guard;
pub mod peer_session;
