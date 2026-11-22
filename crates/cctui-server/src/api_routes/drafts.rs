//! Draft store and spawn-memory routes.

use axum::http::Method;
use axum::routing::get;

use super::GET;
use crate::authz::Routes;
use cctui_server_kernel::authz::Authn;
use cctui_server_kernel::authz::Authz::Authenticated;

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[GET],
        "/drafts",
        "List your unsent drafts.",
        get(cctui_server_sessions::routes::drafts::list_drafts::<crate::state::AppState>),
        Authn::Bearer,
        Authenticated,
    )
    // Wildcard segment: draft keys embed working directories, so they carry slashes.
    .add(
        &[GET, Method::PUT, Method::DELETE],
        "/drafts/{*key}",
        "Read, save or discard one draft.",
        get(cctui_server_sessions::routes::drafts::get_draft::<crate::state::AppState>)
            .put(cctui_server_sessions::routes::drafts::put_draft::<crate::state::AppState>)
            .delete(cctui_server_sessions::routes::drafts::delete_draft::<crate::state::AppState>),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[GET, Method::PUT],
        "/spawn-memory",
        "Get or replace your remembered spawn configuration per target.",
        get(
            cctui_server_sessions::routes::spawn_memory::get_spawn_memory::<crate::state::AppState>,
        )
        .put(
            cctui_server_sessions::routes::spawn_memory::put_spawn_memory::<crate::state::AppState>,
        ),
        Authn::Bearer,
        Authenticated,
    )
}
