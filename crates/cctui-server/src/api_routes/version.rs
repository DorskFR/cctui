//! Version refresh, changelog and self-update routes.

use super::GET;
use crate::authz::Routes;
use crate::routes;
use axum::http::Method;
use axum::routing::{get, post};
use cctui_server_kernel::auth;
use cctui_server_kernel::authz::Authn;
use cctui_server_kernel::authz::Authz::{Authenticated, Scope as ScopeAz};

pub(super) fn register(r: Routes) -> Routes {
    r
        .add(
            &[Method::POST],
            "/version/refresh",
            "Probe upstream for a newer release now instead of waiting out the background interval.",
            post(routes::web::refresh_version),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET],
            "/version/changelog",
            "Release notes of every upstream release newer than this server.",
            get(routes::web::changelog),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::POST],
            "/version/self-update",
            "Deploy the newer release: the machine's own update hook when it has one, a YOLO agent otherwise (admin).",
            post(routes::self_update::launch),
            Authn::Bearer,
            ScopeAz(auth::Scope::Admin),
        )
        .add(
            &[GET],
            "/version/self-update",
            "The most recent update-hook run and where it got to.",
            get(routes::self_update::status),
            Authn::Bearer,
            Authenticated,
        )
}
