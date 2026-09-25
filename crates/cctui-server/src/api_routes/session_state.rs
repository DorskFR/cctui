//! Per-session brief, fork, archive, pin, keepalive and policy routes.

use super::{GET, sess_read, sess_write};
use crate::authz::{Authn, Routes};
use crate::routes;
use axum::http::Method;
use axum::routing::{get, post};

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[Method::GET],
        "/sessions/{id}/brief",
        "Render a session's user/assistant transcript as a capped markdown brief.",
        get(brief::session_brief),
        Authn::Bearer,
        sess_read(),
    )
    .add(
        &[Method::POST],
        "/sessions/{id}/fork",
        "Fork a session into a new one.",
        post(routes::sessions::fork_session),
        Authn::Bearer,
        sess_write(),
    )
    .add(
        &[Method::POST],
        "/sessions/{id}/auto-approve",
        "Toggle auto-approval of tool-use for a session.",
        post(routes::sessions::set_auto_approve),
        Authn::Bearer,
        sess_write(),
    )
    .add(
        &[Method::POST],
        "/sessions/{id}/archive",
        "Archive a single session.",
        post(routes::sessions::archive_session),
        Authn::Bearer,
        sess_write(),
    )
    .add(
        &[Method::POST],
        "/sessions/{id}/unarchive",
        "Unarchive a single session.",
        post(routes::sessions::unarchive_session),
        Authn::Bearer,
        sess_write(),
    )
    .add(
        &[Method::POST],
        "/sessions/{id}/pin",
        "Pin a single session.",
        post(routes::sessions::pin_session),
        Authn::Bearer,
        sess_write(),
    )
    .add(
        &[Method::POST],
        "/sessions/{id}/unpin",
        "Unpin a single session.",
        post(routes::sessions::unpin_session),
        Authn::Bearer,
        sess_write(),
    )
    .add(
        &[Method::POST],
        "/sessions/{id}/keepalive",
        "Set or clear the session's prompt-cache keep-alive schedule.",
        post(routes::sessions::set_keepalive),
        Authn::Bearer,
        sess_write(),
    )
    .add(
        &[Method::POST],
        "/sessions/{id}/policy",
        "Set a session's permission policy.",
        post(routes::sessions::set_session_policy),
        Authn::Bearer,
        sess_write(),
    )
}
