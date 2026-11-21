//! Shared data access for identity, provider accounts and execution.
//!
//! Schema provisioning belongs to the host application. Queries accept a pool
//! or transaction so both server distributions enforce the same invariants.

pub mod account_pools;
pub mod account_providers;
pub mod account_redirects;
pub use cctui_server_kernel::store::acls;
pub mod dispatch_records;
pub mod spawn_capabilities;
pub mod tokens;

pub use cctui_server_kernel::store::shares;

pub mod sessions;

pub mod dispatch_policies;
