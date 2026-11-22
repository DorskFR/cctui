//! Shared data access for identity, provider accounts and execution.
//!
//! Schema provisioning belongs to the host application. Queries accept a pool
//! or transaction so both server distributions enforce the same invariants.

pub use cctui_server_accounts::store::{account_pools, account_providers, account_redirects};
pub use cctui_server_kernel::store::acls;
pub mod dispatch_records;
pub use cctui_server_kernel::store::{spawn_capabilities, tokens};

pub use cctui_server_kernel::store::shares;

pub use cctui_server_kernel::store::sessions;

pub mod dispatch_policies;
