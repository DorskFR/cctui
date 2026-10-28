//! Shared server services for automation and the interactive workspace.
//!
//! This crate must never depend on the full server or workspace services.
//! The initial extraction owns identity and the execution data-access layer;
//! routing, migration orchestration and execution services follow separately.

pub mod account_pick;
pub mod account_resolve;
pub mod auth;
pub mod gateway;
pub mod store;

#[cfg(test)]
mod test_support;
