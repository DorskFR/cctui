//! Typed data-access functions over `&PgPool` / `impl PgExecutor`. Each fn owns
//! one query so invariants that are easy to forget in ad-hoc SQL — a token's
//! `revoked_at IS NULL` guard, a provider mutation's owner + `NOT managed`
//! predicate — live in exactly one place.

pub mod acls;
pub mod sessions;
pub mod spawn_capabilities;
pub mod tokens;
