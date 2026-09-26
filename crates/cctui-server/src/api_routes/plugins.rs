//! Runtime plugin routes.

use super::GET;
use crate::authz::Authz::{Authenticated, Scope as ScopeAz};
use crate::authz::{Authn, Routes};
use crate::{auth, routes};
use axum::extract::DefaultBodyLimit;
use axum::http::Method;
use axum::routing::{get, patch, post};

const INSTALL_BODY_LIMIT: usize = crate::plugin_archive::MAX_ARCHIVE_BYTES + 1024 * 1024;

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[GET],
        "/plugins",
        "List installed plugins with the caller's enabled flags.",
        get(routes::plugins::list),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[Method::POST],
        "/plugins/rescan",
        "Re-read the plugins directory (admin).",
        post(routes::plugins::rescan),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
    .add(
        &[GET, Method::POST],
        "/admin/plugins",
        "List every plugin with its source and instance toggle, or install one from an https URL (JSON `{url}`) or a multipart `file` upload (admin).",
        get(routes::plugins_admin::list)
            .post(routes::plugins_admin::install)
            .layer(DefaultBodyLimit::max(INSTALL_BODY_LIMIT)),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
    .add(
        &[Method::PATCH, Method::DELETE],
        "/admin/plugins/{id}",
        "Enable or disable an installed plugin instance-wide, or uninstall it (admin).",
        patch(routes::plugins_admin::set_enabled).delete(routes::plugins_admin::uninstall),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
}
