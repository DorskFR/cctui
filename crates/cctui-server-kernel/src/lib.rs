//! Foundations shared by every server crate: delivery bus, auth and
//! authorization, errors, schema coordination, adapter normalization and
//! outbound HTTP. Depends on no other server crate.

pub mod accounts_types;
pub mod auth;
pub mod authz;
pub mod blobs_types;
pub mod bus;
pub mod client;
pub mod client_addr;
pub mod commands;
pub mod config;
pub mod context;
pub mod cost;
pub mod crypto;
pub mod dispatch_types;
pub mod envelope_guard;
pub mod error;
pub mod events;
pub mod execution_settings;
pub mod extras_types;
pub mod gateway_trace;
pub mod headers;
pub mod http_cache;
mod live_sessions;
pub mod normalize;
pub mod notify_types;
pub mod outbound;
pub mod peer_limits;
pub mod peer_session;
pub mod preview_host;
pub mod registry;
pub mod resource_owners;
pub mod schema;
pub mod sessions_types;
pub mod state_events;
pub mod store;

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
