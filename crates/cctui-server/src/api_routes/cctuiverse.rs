//! cctuiverse link management. Session routes ride the session guard and also
//! refuse machine keys in the handler; the rest are `Human` and scope every
//! query to the caller.

use super::{GET, sess_read, sess_write};
use crate::authz::Routes;
use axum::http::Method;
use axum::routing::{get, patch, post};
use cctui_server_extras::routes::cctuiverse as h;
use cctui_server_kernel::authz::Authn;
use cctui_server_kernel::authz::Authz::{Authenticated, Human};

const POST: Method = Method::POST;

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[GET],
        "/cctuiverse/config",
        "Whether cctuiverse links are enabled on this server.",
        get(h::config::<crate::state::AppState>),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[POST],
        "/cctuiverse/join",
        "Join a cctuiverse invite with one of your sessions.",
        post(h::join::<crate::state::AppState>),
        Authn::Bearer,
        Human,
    )
    .add(
        &[Method::PATCH],
        "/cctuiverse/links/{id}",
        "Change a cctuiverse link's settings.",
        patch(h::update::<crate::state::AppState>),
        Authn::Bearer,
        Human,
    )
    .add(
        &[POST],
        "/cctuiverse/links/{id}/close",
        "Close a cctuiverse link, or revoke a pending invite.",
        post(h::close::<crate::state::AppState>),
        Authn::Bearer,
        Human,
    )
    .add(
        &[GET],
        "/cctuiverse/links/{id}/messages",
        "Held inbound and review-pending outbound messages of a link.",
        get(h::messages::<crate::state::AppState>),
        Authn::Bearer,
        Human,
    )
    .add(
        &[POST],
        "/cctuiverse/links/{id}/messages/{msg}/approve",
        "Send a review-pending outbound message.",
        post(h::approve::<crate::state::AppState>),
        Authn::Bearer,
        Human,
    )
    .add(
        &[POST],
        "/cctuiverse/links/{id}/messages/{msg}/drop",
        "Discard a held or review-pending message.",
        post(h::drop_message::<crate::state::AppState>),
        Authn::Bearer,
        Human,
    )
    .add(
        &[POST],
        "/cctuiverse/links/{id}/messages/{msg}/release",
        "Deliver a held inbound message now.",
        post(h::release::<crate::state::AppState>),
        Authn::Bearer,
        Human,
    )
    .add(
        &[POST],
        "/rooms/{id}/cctuiverse/invites",
        "Invite a session on another cctui into this room.",
        post(h::invite_room::<crate::state::AppState>),
        Authn::Bearer,
        Human,
    )
    .add(
        &[GET],
        "/rooms/{id}/cctuiverse/links",
        "This room's cctuiverse links, pending invites included.",
        get(h::room_links::<crate::state::AppState>),
        Authn::Bearer,
        Human,
    )
    .add(
        &[POST],
        "/sessions/{id}/cctuiverse/invites",
        "Invite a session on another cctui to link with this one.",
        post(h::invite_session::<crate::state::AppState>),
        Authn::Bearer,
        sess_write(),
    )
    .add(
        &[GET],
        "/sessions/{id}/cctuiverse/links",
        "This session's cctuiverse links, pending invites included.",
        get(h::session_links::<crate::state::AppState>),
        Authn::Bearer,
        sess_read(),
    )
}
