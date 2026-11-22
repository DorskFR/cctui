//! Dispatch and dispatcher-enrolment routes.

use super::GET;
use crate::auth;
use crate::authz::Authz::{Authenticated, Scope as ScopeAz};
use crate::authz::{Authn, Routes};
use axum::http::Method;
use axum::routing::{get, patch, post};

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[Method::POST],
        "/sessions/dispatch",
        "Dispatch a session to an enrolled executor (remote runner).",
        post(cctui_server_dispatch::routes::dispatch::dispatch::<crate::state::AppState>),
        Authn::Bearer,
        ScopeAz(auth::Scope::Dispatch),
    )
    .add(
        &[GET],
        "/sessions/dispatchers",
        "List dispatch targets available for a spawn.",
        get(cctui_server_dispatch::routes::dispatch::list_dispatchers::<crate::state::AppState>),
        Authn::Bearer,
        // owner_filter() SQL filter in the handler.
        Authenticated,
    )
    .add(
        &[GET],
        "/dispatch-records/failed",
        "Dispatches the dispatcher refused or whose worker never ran to a session.",
        get(cctui_server_dispatch::routes::dispatch::list_failed_dispatches::<
            crate::state::AppState,
        >),
        Authn::Bearer,
        // owner_filter() SQL filter in the handler.
        Authenticated,
    )
    .add(
        &[Method::POST],
        "/dispatch-records/{id}/redispatch",
        "Send a recorded dispatch again as a new dispatch.",
        post(cctui_server_dispatch::routes::dispatch::redispatch::<crate::state::AppState>),
        Authn::Bearer,
        ScopeAz(auth::Scope::Dispatch),
    )
    .add(
        &[Method::POST],
        "/dispatch-records/{id}/dismiss",
        "Hide a failed dispatch from the caller's list.",
        post(
            cctui_server_dispatch::routes::dispatch::dismiss_failed_dispatch::<
                crate::state::AppState,
            >,
        ),
        Authn::Bearer,
        // owner_filter() SQL filter in the handler.
        Authenticated,
    )
    .add(
        &[Method::POST],
        "/dispatch-records/dismiss-all",
        "Hide every failed dispatch the caller's list shows.",
        post(
            cctui_server_dispatch::routes::dispatch::dismiss_all_failed_dispatches::<
                crate::state::AppState,
            >,
        ),
        Authn::Bearer,
        // owner_filter() SQL filter in the handler.
        Authenticated,
    )
    // Enrolled-dispatcher management: list with liveness, rename,
    // remove. Enrollment itself is `POST /dispatcher/enroll` below.
    .add(
        &[GET],
        "/dispatchers",
        "List enrolled dispatchers with liveness.",
        get(cctui_server_dispatch::targets::dispatcher_management::list_dispatchers::<
            crate::state::AppState,
        >),
        Authn::Bearer,
        // owner_filter() filter.
        Authenticated,
    )
    .add(
        &[Method::PATCH, Method::DELETE],
        "/dispatchers/{id}",
        "Rename or remove an enrolled dispatcher.",
        patch(
            cctui_server_dispatch::targets::dispatcher_management::update_dispatcher::<
                crate::state::AppState,
            >,
        )
        .delete(
            cctui_server_dispatch::targets::dispatcher_management::delete_dispatcher::<
                crate::state::AppState,
            >,
        ),
        Authn::Bearer,
        ScopeAz(auth::Scope::Enroll),
    )
    .add(
        &[Method::POST],
        "/dispatcher/enroll",
        "Enroll a new dispatcher (executor) and mint its key.",
        post(cctui_server_dispatch::targets::dispatcher::enroll::<crate::state::AppState>),
        Authn::Bearer,
        ScopeAz(auth::Scope::Enroll),
    )
}
