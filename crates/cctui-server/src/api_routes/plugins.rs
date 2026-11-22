//! Runtime plugin routes.

use super::GET;
use crate::authz::Routes;
use axum::extract::DefaultBodyLimit;
use axum::http::Method;
use axum::routing::{get, patch, post};
use cctui_server_kernel::auth;
use cctui_server_kernel::authz::Authn;
use cctui_server_kernel::authz::Authz::{Authenticated, Scope as ScopeAz};

const INSTALL_BODY_LIMIT: usize =
    cctui_server_extras::plugin_archive::MAX_ARCHIVE_BYTES + 1024 * 1024;

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[GET],
        "/plugins",
        "List installed plugins with the caller's enabled flags.",
        get(cctui_server_extras::routes::plugins::list::<crate::state::AppState>),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[GET, Method::POST, Method::PUT, Method::PATCH, Method::DELETE],
        "/plugins/{id}/backend/{*path}",
        "Proxy a request to the plugin's own backend, authenticated as the caller with signed identity headers; the plugin must be instance-enabled and enabled by the caller.",
        // Explicit methods, not `any`: `any` sets only the MethodRouter's
        // fallback, and `route_layer` — which attaches the authz policy — panics
        // unless a named method slot is filled.
        get(cctui_server_extras::plugin_proxy::backend::<crate::state::AppState>)
            .post(cctui_server_extras::plugin_proxy::backend::<crate::state::AppState>)
            .put(cctui_server_extras::plugin_proxy::backend::<crate::state::AppState>)
            .patch(cctui_server_extras::plugin_proxy::backend::<crate::state::AppState>)
            .delete(cctui_server_extras::plugin_proxy::backend::<crate::state::AppState>)
            .layer(DefaultBodyLimit::disable()),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[Method::POST],
        "/plugins/rescan",
        "Re-read the plugins directory (admin).",
        post(cctui_server_extras::routes::plugins::rescan::<crate::state::AppState>),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
    .add(
        &[GET, Method::POST],
        "/admin/plugins",
        "List every plugin with its source and instance toggle, or install one from a published catalog id (JSON `{catalog}`), an https URL (JSON `{url}`) or a multipart `file` upload (admin).",
        get(cctui_server_extras::routes::plugins_admin::list::<crate::state::AppState>)
            .post(cctui_server_extras::routes::plugins_admin::install::<crate::state::AppState>)
            .layer(DefaultBodyLimit::max(INSTALL_BODY_LIMIT)),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
    .add(
        &[GET],
        "/admin/plugins/catalog",
        "List the published plugin catalog, annotated with what this instance has installed (admin).",
        get(cctui_server_extras::routes::plugins_admin::catalog::<crate::state::AppState>),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
    .add(
        &[Method::PATCH, Method::DELETE],
        "/admin/plugins/{id}",
        "Enable or disable an installed plugin instance-wide, or uninstall it (admin).",
        patch(cctui_server_extras::routes::plugins_admin::set_enabled::<crate::state::AppState>).delete(cctui_server_extras::routes::plugins_admin::uninstall::<crate::state::AppState>),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
    .add(
        &[GET, Method::PUT],
        "/admin/plugins/{id}/settings",
        "Read or write a plugin's instance-level settings; secret values are never returned, only whether each is set (admin).",
        get(cctui_server_extras::routes::plugins_admin::get_settings::<crate::state::AppState>).put(cctui_server_extras::routes::plugins_admin::put_settings::<crate::state::AppState>),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
    .add(
        &[Method::POST],
        "/admin/plugins/{id}/proxy-secret",
        "Rotate the plugin's backend-proxy signing secret and return the new value once (admin).",
        post(cctui_server_extras::routes::plugins_admin::rotate_proxy_secret::<crate::state::AppState>),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
}
