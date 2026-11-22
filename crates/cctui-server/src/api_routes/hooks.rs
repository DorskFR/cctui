use axum::http::Method;
use axum::routing::{get, post};

use crate::authz::Routes;
use cctui_server_kernel::authz::Authn;
use cctui_server_kernel::authz::Authz::{Authenticated, Public};

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[Method::POST],
        "/hooks/{id}",
        "Deliver a signed webhook: its HMAC is the only credential.",
        cctui_server_sessions::routes::hooks::receive_route(),
        Authn::None,
        Public,
    )
    .add(
        &[Method::GET, Method::POST],
        "/webhooks",
        "List your inbound webhooks, or create one and read its secret once.",
        get(cctui_server_sessions::routes::hooks::list::<crate::state::AppState>)
            .post(cctui_server_sessions::routes::hooks::create::<crate::state::AppState>),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[Method::GET, Method::PUT, Method::DELETE],
        "/webhooks/{id}",
        "Read, replace or delete one inbound webhook.",
        get(cctui_server_sessions::routes::hooks::get_one::<crate::state::AppState>)
            .put(cctui_server_sessions::routes::hooks::update::<crate::state::AppState>)
            .delete(cctui_server_sessions::routes::hooks::delete::<crate::state::AppState>),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[Method::POST],
        "/webhooks/{id}/secret",
        "Replace a webhook's signing secret and read the new one once.",
        post(cctui_server_sessions::routes::hooks::rotate_secret::<crate::state::AppState>),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[Method::GET],
        "/webhooks/{id}/deliveries",
        "A webhook's delivery log: dispatched, duplicate, rejected, rate limited.",
        get(cctui_server_sessions::routes::hooks::deliveries::<crate::state::AppState>),
        Authn::Bearer,
        Authenticated,
    )
}
