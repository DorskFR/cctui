//! Room routes, and the peer-share writer that lands with them.
//!
//! Rooms carry no `ResourceKind`, so they are `Authenticated` here and every
//! handler scopes its own queries by `user_id`. The per-session routes are
//! ownership-gated by the route guard like the rest of the `/sessions/{id}`
//! family. There is no timeline route: the room has no human-facing page.

use super::{GET, sess_write};
use crate::authz::Authz::Authenticated;
use crate::authz::{Authn, Routes};
use crate::routes;
use axum::http::Method;
use axum::routing::get;

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[GET],
        "/rooms",
        "List rooms, for the picker.",
        get(routes::rooms::list_rooms),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[Method::PATCH, Method::DELETE],
        "/rooms/{id}",
        "Rename/archive, or delete a room.",
        axum::routing::patch(routes::rooms::update_room).delete(routes::rooms::delete_room),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[Method::PUT, Method::DELETE],
        "/sessions/{id}/room",
        "Put this session in a room (by id or name), or take it out of one.",
        axum::routing::put(routes::rooms::set_session_room)
            .delete(routes::rooms::clear_session_room),
        Authn::Bearer,
        sess_write(),
    )
    .add(
        &[GET, Method::POST],
        "/sessions/{id}/peer-shares",
        "List or grant peer-addressing shares for this session.",
        get(routes::rooms::list_peer_shares).post(routes::rooms::create_peer_share),
        Authn::Bearer,
        sess_write(),
    )
    .add(
        &[Method::DELETE],
        "/sessions/{id}/peer-shares/{peer_session_id}",
        "Revoke a peer-addressing share.",
        axum::routing::delete(routes::rooms::revoke_peer_share),
        Authn::Bearer,
        sess_write(),
    )
}
