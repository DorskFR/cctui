//! Bulk session archive and pin routes.

use crate::authz::Authz::Authenticated;
use crate::authz::{Authn, Routes};
use crate::routes;
use axum::http::Method;
use axum::routing::post;

pub(super) fn register(r: Routes) -> Routes {
    r
        // Batch session mutations: owner-filtered in the handler
        // (`filter_owned_ids`), so any authenticated principal is allowed and
        // only their own ids are acted on.
        .add(
            &[Method::POST],
            "/sessions/archive",
            "Archive a batch of sessions by id.",
            post(routes::sessions::archive_sessions),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::POST],
            "/sessions/unarchive",
            "Unarchive a batch of sessions by id.",
            post(routes::sessions::unarchive_sessions),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::POST],
            "/sessions/pin",
            "Pin a batch of sessions by id.",
            post(routes::sessions::pin_sessions),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::POST],
            "/sessions/unpin",
            "Unpin a batch of sessions by id.",
            post(routes::sessions::unpin_sessions),
            Authn::Bearer,
            Authenticated,
        )
}
