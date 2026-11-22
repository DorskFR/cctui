//! Session registration, spawn and staged-file routes.

use super::sess_write;
use crate::authz::Routes;
use axum::extract::DefaultBodyLimit;
use axum::http::Method;
use axum::routing::post;
use cctui_server_kernel::auth;
use cctui_server_kernel::authz::Authn;
use cctui_server_kernel::authz::Authz::{Authenticated, Scope as ScopeAz};

fn body_limit() -> usize {
    cctui_server_kernel::config::upload_body_limit() as usize
}

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[Method::POST],
        "/sessions/register",
        "Register a session the daemon just launched.",
        post(cctui_server_sessions::routes::sessions::register::<crate::state::AppState>),
        Authn::Bearer,
        // In-handler: machine key only; binds to that machine and its owner.
        Authenticated,
    )
    .add(
        &[Method::POST],
        "/sessions/{id}/deregister",
        "Deregister a session (mark it gone).",
        post(cctui_server_sessions::routes::sessions::deregister::<crate::state::AppState>),
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
        post(cctui_server_sessions::routes::spawn::spawn_session::<crate::state::AppState>)
            .layer(DefaultBodyLimit::max(body_limit())),
        Authn::Bearer,
        // In-handler: the machine's owner, an admin, or a `spawn` share grantee.
        ScopeAz(auth::Scope::Dispatch),
    )
    .add(
        // Mid-chat file attachments — same multipart shape + caps
        // as spawn, same body-limit ceiling.
        &[Method::POST],
        "/sessions/{id}/files",
        "Attach files to a live session mid-conversation.",
        post(cctui_server_sessions::routes::spawn::stage_session_files::<crate::state::AppState>)
            .layer(DefaultBodyLimit::max(body_limit())),
        Authn::Bearer,
        sess_write(),
    )
}
