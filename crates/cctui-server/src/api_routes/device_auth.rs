//! Device-login approval. The device-facing `start`/`poll` pair is
//! unauthenticated and lives in `outer_routes`, beside the other self-auth
//! endpoints; only these two need a signed-in user.

use super::GET;
use crate::authz::Routes;
use axum::http::Method;
use axum::routing::{get, post};
use cctui_server_kernel::authz::Authn;
use cctui_server_kernel::authz::Authz::Authenticated;

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[GET],
        "/auth/device/{user_code}",
        "What a pending device login is asking for.",
        get(cctui_server_accounts::routes::device_auth::info::<crate::state::AppState>),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[Method::POST],
        "/auth/device/{user_code}/decision",
        "Approve or deny a device login, granting it your scopes.",
        post(cctui_server_accounts::routes::device_auth::decide::<crate::state::AppState>),
        Authn::Bearer,
        Authenticated,
    )
}
