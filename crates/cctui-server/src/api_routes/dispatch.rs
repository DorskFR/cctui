//! Dispatch and dispatcher-enrolment routes.

use super::GET;
use crate::authz::Authz::{Authenticated, Scope as ScopeAz};
use crate::authz::{Authn, Routes};
use crate::{auth, routes};
use axum::http::Method;
use axum::routing::{get, patch, post};

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[Method::POST],
        "/sessions/dispatch",
        "Dispatch a session to an enrolled executor (remote runner).",
        post(routes::dispatch::dispatch),
        Authn::Bearer,
        ScopeAz(auth::Scope::Dispatch),
    )
    .add(
        &[GET],
        "/sessions/dispatchers",
        "List dispatch targets available for a spawn.",
        get(routes::dispatch::list_dispatchers),
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
        get(routes::dispatchers::list_dispatchers),
        Authn::Bearer,
        // owner_filter() filter.
        Authenticated,
    )
    .add(
        &[Method::PATCH, Method::DELETE],
        "/dispatchers/{id}",
        "Rename or remove an enrolled dispatcher.",
        patch(routes::dispatchers::update_dispatcher)
            .delete(routes::dispatchers::delete_dispatcher),
        Authn::Bearer,
        ScopeAz(auth::Scope::Enroll),
    )
    .add(
        &[Method::POST],
        "/dispatcher/enroll",
        "Enroll a new dispatcher (executor) and mint its key.",
        post(routes::dispatcher::enroll),
        Authn::Bearer,
        ScopeAz(auth::Scope::Enroll),
    )
}
