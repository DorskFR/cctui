use axum::http::Method;
use axum::routing::{get, post};

use crate::authz::Authz::Human;
use crate::authz::{Authn, Routes};
use crate::routes;

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[Method::GET],
        "/notifications",
        "List your notifications, open or all, with the unread count.",
        get(routes::notifications::list),
        Authn::Bearer,
        Human,
    )
    .add(
        &[Method::POST],
        "/notifications/read-all",
        "Mark every unread notification without actions read.",
        post(routes::notifications::read_all),
        Authn::Bearer,
        Human,
    )
    .add(
        &[Method::POST],
        "/notifications/{id}/read",
        "Mark one notification read.",
        post(routes::notifications::read),
        Authn::Bearer,
        Human,
    )
    .add(
        &[Method::POST],
        "/notifications/{id}/act",
        "Answer a notification with one of its actions; the first answer wins.",
        post(routes::notifications::act),
        Authn::Bearer,
        Human,
    )
    .add(
        &[Method::POST],
        "/notifications/{id}/dismiss",
        "Dismiss a notification; refused for a blocking approval.",
        post(routes::notifications::dismiss),
        Authn::Bearer,
        Human,
    )
}
