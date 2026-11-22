//! Version info and passkey routes.

use super::GET;
use crate::authz::Authz::{Authenticated, Public, Scope as ScopeAz};
use crate::authz::{Authn, Routes};
use crate::{auth, routes};
use axum::http::Method;
use axum::routing::{get, patch, post, put};

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
        get(routes::passkeys::list::<crate::state::AppState>),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[Method::POST],
        "/passkeys/register/start",
        "Begin enrolling a passkey on your account.",
        post(routes::passkeys::register_start::<crate::state::AppState>),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[Method::POST],
        "/passkeys/register/finish",
        "Finish enrolling a passkey and store the credential.",
        post(routes::passkeys::register_finish::<crate::state::AppState>),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[Method::POST],
        "/passkeys/test/start",
        "Begin a test of an enrolled passkey without signing out.",
        post(routes::passkeys::test_start::<crate::state::AppState>),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[Method::POST],
        "/passkeys/test/finish",
        "Finish a passkey test and report which key answered.",
        post(routes::passkeys::test_finish::<crate::state::AppState>),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[Method::PATCH, Method::DELETE],
        "/passkeys/{id}",
        "Rename or revoke one of your passkeys.",
        patch(routes::passkeys::relabel::<crate::state::AppState>)
            .delete(routes::passkeys::revoke::<crate::state::AppState>),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[Method::PUT],
        "/admin/passkeys/auto-prompt",
        "Server-wide: try the passkey as soon as the login screen opens (admin).",
        put(routes::passkeys::set_auto_prompt::<crate::state::AppState>),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
}
