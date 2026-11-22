//! Per-session messaging, scheduling, pins and read-state routes.

use super::{GET, sess_read, sess_write};
use crate::authz::Routes;
use axum::http::Method;
use axum::routing::{get, patch, post, put};
use cctui_server_kernel::authz::Authn;

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[Method::POST],
        "/sessions/{id}/message",
        "Send a message to a live session.",
        post(cctui_server_sessions::routes::sessions::send_message::<crate::state::AppState>),
        Authn::Bearer,
        sess_write(),
    )
    .add(
        &[Method::POST],
        "/sessions/{id}/followup",
        "Continue a session in a new one and swap it in once it registers.",
        post(cctui_server_sessions::followup_op::create::<crate::state::AppState>),
        Authn::Bearer,
        sess_write(),
    )
    .add(
        &[Method::PUT],
        "/sessions/{id}/auto-followup",
        "Set or clear a session's auto follow-up override.",
        put(cctui_server_sessions::auto_followup::put_override::<crate::state::AppState>),
        Authn::Bearer,
        sess_write(),
    )
    .add(
        &[GET],
        "/sessions/{id}/messages/scheduled",
        "List a session's scheduled messages.",
        get(cctui_server_sessions::routes::scheduled_messages::list::<crate::state::AppState>),
        Authn::Bearer,
        sess_read(),
    )
    .add(
        &[Method::PATCH, Method::DELETE],
        "/sessions/{id}/messages/scheduled/{queue_id}",
        "Edit/reschedule or cancel a scheduled message.",
        patch(cctui_server_sessions::routes::scheduled_messages::update::<crate::state::AppState>)
            .delete(
                cctui_server_sessions::routes::scheduled_messages::cancel::<crate::state::AppState>,
            ),
        Authn::Bearer,
        sess_write(),
    )
    .add(
        &[Method::POST],
        "/sessions/{id}/messages/scheduled/{queue_id}/send-now",
        "Deliver a scheduled message immediately.",
        post(cctui_server_sessions::routes::scheduled_messages::send_now::<crate::state::AppState>),
        Authn::Bearer,
        sess_write(),
    )
    .add(
        &[Method::POST],
        "/sessions/{id}/kill",
        "Kill a session's underlying process.",
        post(cctui_server_sessions::routes::sessions::kill_session::<crate::state::AppState>),
        Authn::Bearer,
        sess_write(),
    )
    .add(
        &[GET],
        "/sessions/{id}/pins",
        "List the caller's pinned messages in a session.",
        get(cctui_server_sessions::routes::message_pins::list_pins::<crate::state::AppState>),
        Authn::Bearer,
        sess_read(),
    )
    .add(
        &[Method::POST],
        "/sessions/{id}/pins",
        "Pin a message (by stream seq) in a session.",
        post(cctui_server_sessions::routes::message_pins::create_pin::<crate::state::AppState>),
        Authn::Bearer,
        sess_write(),
    )
    .add(
        &[Method::DELETE],
        "/sessions/{id}/pins/{seq}",
        "Unpin a message in a session.",
        axum::routing::delete(
            cctui_server_sessions::routes::message_pins::delete_pin::<crate::state::AppState>,
        ),
        Authn::Bearer,
        sess_write(),
    )
    .add(
        &[Method::POST],
        "/sessions/{id}/seen",
        "Mark this session's messages seen for the caller.",
        post(cctui_server_sessions::routes::sessions::mark_seen::<crate::state::AppState>),
        Authn::Bearer,
        sess_write(),
    )
}
