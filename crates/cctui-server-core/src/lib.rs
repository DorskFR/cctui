//! Shared server services for automation and the interactive workspace.
//!
//! This crate must never depend on the full server or workspace services.
//! Owns identity, execution persistence, account selection, launch tracking and
//! provider credentials. Hosts supply transport and optional workspace behavior.

pub use cctui_server_accounts::{account_pick, account_resolve, fireworks_billing, gateway};
pub use cctui_server_kernel::{auth, commands, cost};
pub mod store;
pub use cctui_server_accounts::usage_probe;

#[cfg(any(test, feature = "test-support"))]
pub use cctui_server_kernel::test_support;

pub use cctui_server_kernel::bus;
pub mod dispatchers;
pub use cctui_server_kernel::registry;

pub use cctui_server_kernel::{client, outbound};

pub use cctui_server_accounts::hook_policy;
pub mod launch;

pub use cctui_server_accounts::launch_settings;

pub use cctui_server_kernel::events;
pub mod lifecycle;
pub use cctui_server_kernel::{ended_signals, live_sessions_predicate};

pub mod dispatch;
pub mod dispatch_identity;
pub mod dispatch_policy;

pub use cctui_server_accounts::provider_status;

pub use cctui_server_kernel::error;
pub mod machine_spawn;

pub use cctui_server_kernel::execution_settings;

pub use cctui_server_accounts::session_credentials;

pub mod state;

pub use cctui_server_accounts::identity;
pub mod targets;

pub use cctui_server_kernel::normalize;

pub mod results;

pub mod launch_credentials;

pub mod spawn_defaults;

pub use cctui_server_accounts::{accounts, settings_catalog};
pub use cctui_server_kernel::resource_owners;

pub mod machine_adapters;
pub mod machine_policy;

pub mod daemon_checkpoints;
pub use cctui_server_accounts::daemon_usage;

pub mod webhook;

pub mod dispatch_liveness;
pub mod schema;
pub mod worker_retirement;

pub mod machine_launches;

pub mod child_policy;

pub mod child_exec;

pub mod session_limits;

pub use cctui_server_kernel::{envelope_guard, peer_session};
