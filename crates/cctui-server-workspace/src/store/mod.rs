//! Typed data-access functions over `&PgPool` / `impl PgExecutor`. Each fn owns
//! one query so invariants that are easy to forget in ad-hoc SQL — a token's
//! `revoked_at IS NULL` guard, a provider mutation's owner + `NOT managed`
//! predicate — live in exactly one place.

#[cfg(test)]
pub mod acls;
pub use cctui_server_sessions::store::{sessions, tokens};
pub mod spawn_capabilities;
