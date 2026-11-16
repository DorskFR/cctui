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
}
