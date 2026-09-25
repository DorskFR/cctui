//! Per-session messaging, scheduling, pins and read-state routes.

use super::{GET, sess_read, sess_write};
use crate::authz::{Authn, Routes};
use crate::routes;
use axum::http::Method;
use axum::routing::{delete, get, patch, post};

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[Method::POST],
        "/sessions/{id}/message",
        "Send a message to a live session.",
        post(routes::sessions::send_message),
        Authn::Bearer,
        sess_write(),
    )
    .add(
        &[GET],
        "/sessions/{id}/messages/scheduled",
        "List a session's scheduled messages.",
        get(routes::scheduled_messages::list),
        Authn::Bearer,
        sess_read(),
    )
    .add(
        &[Method::PATCH, Method::DELETE],
        "/sessions/{id}/messages/scheduled/{queue_id}",
        "Edit/reschedule or cancel a scheduled message.",
        patch(routes::scheduled_messages::update).delete(routes::scheduled_messages::cancel),
        Authn::Bearer,
        sess_write(),
    )
    .add(
        &[Method::POST],
        "/sessions/{id}/messages/scheduled/{queue_id}/send-now",
        "Deliver a scheduled message immediately.",
        post(routes::scheduled_messages::send_now),
        Authn::Bearer,
        sess_write(),
    )
    .add(
        &[Method::POST],
        "/sessions/{id}/kill",
        "Kill a session's underlying process.",
        post(routes::sessions::kill_session),
        Authn::Bearer,
        sess_write(),
    )
    .add(
        &[GET],
        "/sessions/{id}/pins",
        "List the caller's pinned messages in a session.",
        get(routes::message_pins::list_pins),
        Authn::Bearer,
        sess_read(),
    )
    .add(
        &[Method::POST],
        "/sessions/{id}/pins",
        "Pin a message (by stream seq) in a session.",
        post(routes::message_pins::create_pin),
        Authn::Bearer,
        sess_write(),
    )
    .add(
        &[Method::DELETE],
        "/sessions/{id}/pins/{seq}",
        "Unpin a message in a session.",
        axum::routing::delete(routes::message_pins::delete_pin),
        Authn::Bearer,
        sess_write(),
    )
    .add(
        &[Method::POST],
        "/sessions/{id}/seen",
        "Mark this session's messages seen for the caller.",
        post(routes::sessions::mark_seen),
        Authn::Bearer,
        sess_write(),
    )
}
