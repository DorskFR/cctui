//! Version info and passkey routes.

use super::GET;
use crate::authz::Routes;
use crate::routes;
use axum::http::Method;
use axum::routing::{get, patch, post, put};
use cctui_server_kernel::auth;
use cctui_server_kernel::authz::Authn;
use cctui_server_kernel::authz::Authz::{Authenticated, Public, Scope as ScopeAz};

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[GET],
        "/version",
        "Server version and build info. Public; deployment fields need a token.",
        get(routes::web::version),
        Authn::None,
        Public,
    )
    .add(
        &[GET],
        "/passkeys",
        "List the passkeys enrolled on your account.",
        get(cctui_server_accounts::routes::passkeys::list::<crate::state::AppState>),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[Method::POST],
        "/passkeys/register/start",
        "Begin enrolling a passkey on your account.",
        post(cctui_server_accounts::routes::passkeys::register_start::<crate::state::AppState>),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[Method::POST],
        "/passkeys/register/finish",
        "Finish enrolling a passkey and store the credential.",
        post(cctui_server_accounts::routes::passkeys::register_finish::<crate::state::AppState>),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[Method::POST],
        "/passkeys/test/start",
        "Begin a test of an enrolled passkey without signing out.",
        post(cctui_server_accounts::routes::passkeys::test_start::<crate::state::AppState>),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[Method::POST],
        "/passkeys/test/finish",
        "Finish a passkey test and report which key answered.",
        post(cctui_server_accounts::routes::passkeys::test_finish::<crate::state::AppState>),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[Method::PATCH, Method::DELETE],
        "/passkeys/{id}",
        "Rename or revoke one of your passkeys.",
        patch(cctui_server_accounts::routes::passkeys::relabel::<crate::state::AppState>)
            .delete(cctui_server_accounts::routes::passkeys::revoke::<crate::state::AppState>),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[Method::PUT],
        "/admin/passkeys/auto-prompt",
        "Server-wide: try the passkey as soon as the login screen opens (admin).",
        put(cctui_server_accounts::routes::passkeys::set_auto_prompt::<crate::state::AppState>),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
}
