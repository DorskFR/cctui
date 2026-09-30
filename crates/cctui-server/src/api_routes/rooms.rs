//! Room routes, and the peer-share writer that lands with them.
//!
//! Rooms carry no `ResourceKind`, so they are `Authenticated` here and every
//! handler scopes its own queries by `user_id`. The per-session routes are
//! ownership-gated by the route guard like the rest of the `/sessions/{id}`
//! family.

use super::{GET, sess_read, sess_write};
use crate::authz::Authz::Authenticated;
use crate::authz::{Authn, Routes};
use crate::routes;
use axum::http::Method;
use axum::routing::{get, post};

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[GET, Method::POST],
        "/rooms",
        "List rooms, or create one from the selected sessions.",
        get(routes::rooms::list_rooms).post(routes::rooms::create_room),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[GET, Method::PATCH, Method::DELETE],
        "/rooms/{id}",
        "Read, rename/archive, or delete a room.",
        get(routes::rooms::get_room)
            .patch(routes::rooms::update_room)
            .delete(routes::rooms::delete_room),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[Method::POST],
        "/rooms/{id}/members",
        "Add a session to a room.",
        post(routes::rooms::add_member),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[Method::DELETE],
        "/rooms/{id}/members/{session_id}",
        "Remove a session from a room.",
        axum::routing::delete(routes::rooms::remove_member),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[GET, Method::POST],
        "/rooms/{id}/messages",
        "Read the room timeline, or post to it as the human.",
        get(routes::rooms::get_messages).post(routes::rooms::post_message),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[GET],
        "/sessions/{id}/rooms",
        "Rooms this session belongs to.",
        get(routes::rooms::session_rooms),
        Authn::Bearer,
        sess_read(),
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
