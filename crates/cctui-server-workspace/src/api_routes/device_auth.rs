//! Device-login approval. The device-facing `start`/`poll` pair is
//! unauthenticated and lives in `outer_routes`, beside the other self-auth
//! endpoints; only these two need a signed-in user.

use super::GET;
use crate::authz::Authz::Authenticated;
use crate::authz::{Authn, Routes};
use crate::routes;
use axum::http::Method;
use axum::routing::{get, post};

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[GET],
        "/auth/device/{user_code}",
        "What a pending device login is asking for.",
        get(routes::device_auth::info),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[Method::POST],
        "/auth/device/{user_code}/decision",
        "Approve or deny a device login, granting it your scopes.",
        post(routes::device_auth::decide),
        Authn::Bearer,
        Authenticated,
    )
}
