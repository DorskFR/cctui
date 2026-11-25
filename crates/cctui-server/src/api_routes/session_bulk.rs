//! Bulk session archive and pin routes.

use crate::authz::Routes;
use axum::http::Method;
use axum::routing::post;
use cctui_server_kernel::authz::Authn;
use cctui_server_kernel::authz::Authz::Authenticated;

pub(super) fn register(r: Routes) -> Routes {
    r
        // Batch session mutations: owner-filtered in the handler
        // (`filter_owned_ids`), so any authenticated principal is allowed and
        // only their own ids are acted on.
        .add(
            &[Method::POST],
            "/sessions/archive",
            "Archive a batch of sessions by id.",
            post(cctui_server_sessions::routes::sessions::archive_sessions::<crate::state::AppState>),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::POST],
            "/sessions/unarchive",
            "Unarchive a batch of sessions by id.",
            post(cctui_server_sessions::routes::sessions::unarchive_sessions::<crate::state::AppState>),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::POST],
            "/sessions/pin",
            "Pin a batch of sessions by id.",
            post(cctui_server_sessions::routes::sessions::pin_sessions::<crate::state::AppState>),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::POST],
            "/sessions/unpin",
            "Unpin a batch of sessions by id.",
            post(cctui_server_sessions::routes::sessions::unpin_sessions::<crate::state::AppState>),
            Authn::Bearer,
            Authenticated,
        )
}
