use axum::http::Method;
use axum::routing::{get, post};

use crate::authz::Authz::{Authenticated, Public};
use crate::authz::{Authn, Routes};
use crate::routes;

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[Method::POST],
        "/hooks/{id}",
        "Deliver a signed webhook: its HMAC is the only credential.",
        routes::hooks::receive_route(),
        Authn::None,
        Public,
    )
    .add(
        &[Method::GET, Method::POST],
        "/webhooks",
        "List your inbound webhooks, or create one and read its secret once.",
        get(routes::hooks::list).post(routes::hooks::create),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[Method::GET, Method::PUT, Method::DELETE],
        "/webhooks/{id}",
        "Read, replace or delete one inbound webhook.",
        get(routes::hooks::get_one).put(routes::hooks::update).delete(routes::hooks::delete),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[Method::POST],
        "/webhooks/{id}/secret",
        "Replace a webhook's signing secret and read the new one once.",
        post(routes::hooks::rotate_secret),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[Method::GET],
        "/webhooks/{id}/deliveries",
        "A webhook's delivery log: dispatched, duplicate, rejected, rate limited.",
        get(routes::hooks::deliveries),
        Authn::Bearer,
        Authenticated,
    )
}
