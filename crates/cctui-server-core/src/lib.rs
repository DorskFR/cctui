//! Shared server services for automation and the interactive workspace.
//!
//! This crate must never depend on the full server or workspace services.
//! Owns identity, execution persistence, account selection, launch tracking and
//! provider credentials. Hosts supply transport and optional workspace behavior.

pub mod account_pick;
pub mod account_resolve;
pub mod auth;
pub mod commands;
pub mod cost;
pub mod fireworks_billing;
pub mod gateway;
pub mod store;
pub mod usage_probe;

#[cfg(test)]
mod test_support;

pub mod bus;
pub mod dispatchers;
pub mod policy;
pub mod registry;

pub mod client;
pub mod outbound;

pub mod launch;

pub mod launch_settings;

pub mod events;
pub mod lifecycle;
mod live_sessions;

pub mod dispatch;
pub mod dispatch_policy;

pub mod provider_status;
