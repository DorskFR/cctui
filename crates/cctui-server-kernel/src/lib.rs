//! Foundations shared by every server crate: delivery bus, auth and
//! authorization, errors, schema coordination, adapter normalization and
//! outbound HTTP. Depends on no other server crate.

pub mod auth;
pub mod authz;
pub mod bus;
pub mod client;
pub mod client_addr;
pub mod config;
pub mod context;
pub mod crypto;
pub mod envelope_guard;
pub mod error;
pub mod gateway_trace;
pub mod headers;
pub mod normalize;
pub mod outbound;
pub mod preview_host;
pub mod resource_owners;
pub mod schema;
pub mod store;

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
