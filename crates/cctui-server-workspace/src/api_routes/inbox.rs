use axum::http::Method;
use axum::routing::{get, post};

use crate::authz::Authz::{Authenticated, Scope as ScopeAz};
use crate::authz::{Authn, Routes};
use crate::{auth, routes};

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[Method::POST],
        "/inbox/events",
        "Queue an event into a session's inbox lane, deduplicated by source id.",
        post(routes::inbox::create),
        Authn::Bearer,
        ScopeAz(auth::Scope::Dispatch),
    )
    .add(
        &[Method::GET],
        "/inbox/events",
        "List your inbox events, by lane and status.",
        get(routes::inbox::list),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[Method::GET, Method::DELETE],
        "/inbox/events/{id}",
        "Read one inbox event, or cancel it while pending.",
        get(routes::inbox::get_one).delete(routes::inbox::cancel),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[Method::POST],
        "/inbox/lanes/{id}/pause",
        "Pause an inbox lane: its events wait until it is resumed.",
        post(routes::inbox::pause_lane),
        Authn::Bearer,
        ScopeAz(auth::Scope::Dispatch),
    )
    .add(
        &[Method::POST],
        "/inbox/lanes/{id}/resume",
        "Resume a paused inbox lane and deliver what waited.",
        post(routes::inbox::resume_lane),
        Authn::Bearer,
        ScopeAz(auth::Scope::Dispatch),
    )
    .add(
        &[Method::POST],
        "/inbox/events/{id}/retry",
        "Return a dead inbox event to pending with its attempts reset.",
        post(routes::inbox::retry),
        Authn::Bearer,
        ScopeAz(auth::Scope::Dispatch),
    )
}
