//! Session registration, spawn and staged-file routes.

use super::sess_write;
use crate::authz::Authz::Authenticated;
use crate::authz::{Authn, Routes};
use crate::routes;
use axum::extract::DefaultBodyLimit;
use axum::http::Method;
use axum::routing::post;

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
        // Multipart spawn with file uploads: the route enforces a
        // 20 MB total cap itself; allow a little headroom over it for
        // multipart framing + base64 isn't applied until after parsing.
        post(routes::spawn::spawn_session).layer(DefaultBodyLimit::max(24 * 1024 * 1024)),
        Authn::Bearer,
        // In-handler machine-owner check (`is_admin || user_id == owner`).
        Authenticated,
    )
    .add(
        // Mid-chat file attachments — same multipart shape + caps
        // as spawn, same body-limit headroom.
        &[Method::POST],
        "/sessions/{id}/files",
        "Attach files to a live session mid-conversation.",
        post(routes::spawn::stage_session_files).layer(DefaultBodyLimit::max(24 * 1024 * 1024)),
        Authn::Bearer,
        sess_write(),
    )
}
