//! Permission and skill routes.

use super::GET;
use crate::authz::Routes;
use axum::extract::DefaultBodyLimit;
use axum::http::Method;
use axum::routing::{get, put};
use cctui_server_kernel::authz::Authn;
use cctui_server_kernel::authz::Authz::Authenticated;

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[GET],
        "/permissions/pending",
        "List pending tool-use permission requests.",
        get(cctui_server_sessions::routes::permissions::list_pending::<crate::state::AppState>),
        Authn::Bearer,
        // In-handler owner join on session machine_uuid -> machines.user_id.
        Authenticated,
    )
    .add(
        &[GET],
        "/skills/index",
        "List available skills.",
        get(cctui_server_extras::skills::index),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[Method::PUT, GET],
        "/skills/{name}",
        "Upload or fetch a skill bundle by name.",
        put(cctui_server_extras::skills::put)
            .get(cctui_server_extras::skills::get)
            .layer(DefaultBodyLimit::max(50 * 1024 * 1024)),
        Authn::Bearer,
        Authenticated,
    )
}
