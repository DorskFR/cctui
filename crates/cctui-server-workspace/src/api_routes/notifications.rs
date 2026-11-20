use axum::http::Method;
use axum::routing::{delete, get, post};

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
        &[Method::GET, Method::POST],
        "/notifications/endpoints",
        "List where your notifications reach you, or add a place.",
        get(routes::notification_endpoints::list).post(routes::notification_endpoints::create),
        Authn::Bearer,
        Human,
    )
    .add(
        &[Method::DELETE],
        "/notifications/endpoints/{id}",
        "Remove one of your notification endpoints.",
        delete(routes::notification_endpoints::delete),
        Authn::Bearer,
        Human,
    )
    .add(
        &[Method::POST],
        "/notifications/endpoints/{id}/test",
        "Send a test notification to one endpoint only.",
        post(routes::notification_endpoints::send_test),
        Authn::Bearer,
        Human,
    )
    .add(
        &[Method::GET],
        "/notifications/settings",
        "Your notification settings with every default filled in.",
        get(routes::notification_endpoints::settings),
        Authn::Bearer,
        Human,
    )
    .add(
        &[Method::POST],
        "/notifications/read-all",
        "Mark every unread notification read.",
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
        "Dismiss a notification; one a run waits on needs confirm=true and expires.",
        post(routes::notifications::dismiss),
        Authn::Bearer,
        Human,
    )
}
