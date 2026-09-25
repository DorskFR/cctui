//! Permission and skill routes.

use super::GET;
use crate::authz::Authz::Authenticated;
use crate::authz::{Authn, Routes};
use crate::routes;
use axum::extract::DefaultBodyLimit;
use axum::http::Method;
use axum::routing::{get, put};

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[GET],
        "/permissions/pending",
        "List pending tool-use permission requests.",
        get(routes::permissions::list_pending),
        Authn::Bearer,
        // In-handler owner join on session machine_uuid -> machines.user_id.
        Authenticated,
    )
    .add(
        &[GET],
        "/skills/index",
        "List available skills.",
        get(routes::skills::index),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[Method::PUT, GET],
        "/skills/{name}",
        "Upload or fetch a skill bundle by name.",
        put(routes::skills::put)
            .get(routes::skills::get)
            .layer(DefaultBodyLimit::max(50 * 1024 * 1024)),
        Authn::Bearer,
        Authenticated,
    )
}
