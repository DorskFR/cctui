//! Room routes.
//!
//! Rooms carry no `ResourceKind`, so they are `Authenticated` here and every
//! handler scopes its own queries by `user_id`. The per-session routes are
//! ownership-gated by the route guard like the rest of the `/sessions/{id}`
//! family. There is no timeline route: the room has no human-facing page.

use super::{GET, sess_write};
use crate::authz::Routes;
use axum::http::Method;
use axum::routing::get;
use cctui_server_kernel::authz::Authn;
use cctui_server_kernel::authz::Authz::Authenticated;

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[GET],
        "/rooms",
        "List rooms, for the picker.",
        get(cctui_server_extras::routes::rooms::list_rooms::<crate::state::AppState>),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[Method::PATCH, Method::DELETE],
        "/rooms/{id}",
        "Rename/archive, or delete a room.",
        axum::routing::patch(
            cctui_server_extras::routes::rooms::update_room::<crate::state::AppState>,
        )
        .delete(cctui_server_extras::routes::rooms::delete_room::<crate::state::AppState>),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[Method::PUT, Method::DELETE],
        "/sessions/{id}/room",
        "Put this session in a room (by id or name), or take it out of one.",
        axum::routing::put(
            cctui_server_extras::routes::rooms::set_session_room::<crate::state::AppState>,
        )
        .delete(cctui_server_extras::routes::rooms::clear_session_room::<crate::state::AppState>),
        Authn::Bearer,
        sess_write(),
    )
}
