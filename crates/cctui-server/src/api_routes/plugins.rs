//! Runtime plugin routes.

use super::GET;
use crate::authz::Authz::{Authenticated, Scope as ScopeAz};
use crate::authz::{Authn, Routes};
use crate::{auth, routes};
use axum::extract::DefaultBodyLimit;
use axum::http::Method;
use axum::routing::{any, get, patch, post};

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
        &[GET, Method::POST, Method::PUT, Method::PATCH, Method::DELETE],
        "/plugins/{id}/backend/{*path}",
        "Proxy a request to the plugin's own backend, authenticated as the caller with signed identity headers; the plugin must be instance-enabled and enabled by the caller.",
        any(crate::plugin_proxy::backend).layer(DefaultBodyLimit::disable()),
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
        "List every plugin with its source and instance toggle, or install one from a published catalog id (JSON `{catalog}`), an https URL (JSON `{url}`) or a multipart `file` upload (admin).",
        get(routes::plugins_admin::list)
            .post(routes::plugins_admin::install)
            .layer(DefaultBodyLimit::max(INSTALL_BODY_LIMIT)),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
    .add(
        &[GET],
        "/admin/plugins/catalog",
        "List the published plugin catalog, annotated with what this instance has installed (admin).",
        get(routes::plugins_admin::catalog),
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
    .add(
        &[GET, Method::PUT],
        "/admin/plugins/{id}/settings",
        "Read or write a plugin's instance-level settings; secret values are never returned, only whether each is set (admin).",
        get(routes::plugins_admin::get_settings).put(routes::plugins_admin::put_settings),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
    .add(
        &[Method::POST],
        "/admin/plugins/{id}/proxy-secret",
        "Rotate the plugin's backend-proxy signing secret and return the new value once (admin).",
        post(routes::plugins_admin::rotate_proxy_secret),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
}
