//! Draft store and spawn-memory routes.

use axum::http::Method;
use axum::routing::get;

use super::GET;
use crate::authz::Authz::Authenticated;
use crate::authz::{Authn, Routes};
use crate::routes;

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[GET],
        "/drafts",
        "List your unsent drafts.",
        get(routes::drafts::list_drafts::<crate::state::AppState>),
        Authn::Bearer,
        Authenticated,
    )
    // Wildcard segment: draft keys embed working directories, so they carry slashes.
    .add(
        &[GET, Method::PUT, Method::DELETE],
        "/drafts/{*key}",
        "Read, save or discard one draft.",
        get(routes::drafts::get_draft::<crate::state::AppState>)
            .put(routes::drafts::put_draft::<crate::state::AppState>)
            .delete(routes::drafts::delete_draft::<crate::state::AppState>),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[GET, Method::PUT],
        "/spawn-memory",
        "Get or replace your remembered spawn configuration per target.",
        get(routes::spawn_memory::get_spawn_memory::<crate::state::AppState>)
            .put(routes::spawn_memory::put_spawn_memory::<crate::state::AppState>),
        Authn::Bearer,
        Authenticated,
    )
}
