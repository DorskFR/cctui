use axum::http::Method;
use axum::routing::{delete, get, post};

use crate::authz::Routes;
use cctui_server_kernel::authz::Authn;
use cctui_server_kernel::authz::Authz::Human;

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[Method::GET],
        "/notifications",
        "List your notifications, open or all, with the unread count.",
        get(cctui_server_notify::routes::notifications::list::<crate::state::AppState>),
        Authn::Bearer,
        Human,
    )
    .add(
        &[Method::GET, Method::POST],
        "/notifications/endpoints",
        "List where your notifications reach you, or add a place.",
        get(cctui_server_notify::routes::notification_endpoints::list::<crate::state::AppState>)
            .post(cctui_server_notify::routes::notification_endpoints::create::<crate::state::AppState>),
        Authn::Bearer,
        Human,
    )
    .add(
        &[Method::DELETE],
        "/notifications/endpoints/{id}",
        "Remove one of your notification endpoints.",
        delete(cctui_server_notify::routes::notification_endpoints::delete::<crate::state::AppState>),
        Authn::Bearer,
        Human,
    )
    .add(
        &[Method::POST],
        "/notifications/endpoints/{id}/test",
        "Send a test notification to one endpoint only.",
        post(cctui_server_notify::routes::notification_endpoints::send_test::<crate::state::AppState>),
        Authn::Bearer,
        Human,
    )
    .add(
        &[Method::GET],
        "/notifications/settings",
        "Your notification settings with every default filled in.",
        get(cctui_server_notify::routes::notification_endpoints::settings::<crate::state::AppState>),
        Authn::Bearer,
        Human,
    )
    .add(
        &[Method::POST],
        "/notifications/read-all",
        "Mark every unread notification read.",
        post(cctui_server_notify::routes::notifications::read_all::<crate::state::AppState>),
        Authn::Bearer,
        Human,
    )
    .add(
        &[Method::POST],
        "/notifications/{id}/read",
        "Mark one notification read.",
        post(cctui_server_notify::routes::notifications::read::<crate::state::AppState>),
        Authn::Bearer,
        Human,
    )
    .add(
        &[Method::POST],
        "/notifications/{id}/act",
        "Answer a notification with one of its actions; the first answer wins.",
        post(cctui_server_notify::routes::notifications::act::<crate::state::AppState>),
        Authn::Bearer,
        Human,
    )
    .add(
        &[Method::POST],
        "/notifications/{id}/dismiss",
        "Dismiss a notification; one a run waits on needs confirm=true and expires.",
        post(cctui_server_notify::routes::notifications::dismiss::<crate::state::AppState>),
        Authn::Bearer,
        Human,
    )
}
