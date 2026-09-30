//! Session registration, spawn and staged-file routes.

use super::sess_write;
use crate::authz::Authz::Authenticated;
use crate::authz::{Authn, Routes};
use crate::routes;
use axum::extract::DefaultBodyLimit;
use axum::http::Method;
use axum::routing::post;

fn body_limit() -> usize {
    crate::config::upload_body_limit() as usize
}

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[Method::POST],
        "/sessions/register",
        "Register a session the daemon just launched.",
        post(routes::sessions::register),
        Authn::Bearer,
        // In-handler: machine key only; binds to that machine and its owner.
        Authenticated,
    )
    .add(
        &[Method::POST],
        "/sessions/{id}/deregister",
        "Deregister a session (mark it gone).",
        post(routes::sessions::deregister),
        Authn::Bearer,
        sess_write(),
    )
    .add(
        &[Method::POST],
        "/sessions/spawn",
        "Spawn a new session on a machine, with optional file uploads.",
        // Multipart spawn with file uploads: the route enforces the configured
        // total cap itself, which must stay strictly below this ceiling so an
        // over-cap upload yields its 413 rather than a body-limit error.
        post(routes::spawn::spawn_session).layer(DefaultBodyLimit::max(body_limit())),
        Authn::Bearer,
        // In-handler machine-owner check (`is_admin || user_id == owner`).
        Authenticated,
    )
    .add(
        // Mid-chat file attachments — same multipart shape + caps
        // as spawn, same body-limit ceiling.
        &[Method::POST],
        "/sessions/{id}/files",
        "Attach files to a live session mid-conversation.",
        post(routes::spawn::stage_session_files).layer(DefaultBodyLimit::max(body_limit())),
        Authn::Bearer,
        sess_write(),
    )
}
