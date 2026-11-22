use axum::http::Method;
use axum::routing::{get, post};

use crate::authz::Routes;
use cctui_server_kernel::auth;
use cctui_server_kernel::authz::Authn;
use cctui_server_kernel::authz::Authz::{Authenticated, Scope as ScopeAz};

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[Method::POST],
        "/inbox/events",
        "Queue an event into a session's inbox lane, deduplicated by source id.",
        post(cctui_server_sessions::routes::inbox::create::<crate::state::AppState>),
        Authn::Bearer,
        ScopeAz(auth::Scope::Dispatch),
    )
    .add(
        &[Method::GET],
        "/inbox/events",
        "List your inbox events, by lane and status.",
        get(cctui_server_sessions::routes::inbox::list::<crate::state::AppState>),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[Method::GET, Method::DELETE],
        "/inbox/events/{id}",
        "Read one inbox event, or cancel it while pending.",
        get(cctui_server_sessions::routes::inbox::get_one::<crate::state::AppState>)
            .delete(cctui_server_sessions::routes::inbox::cancel::<crate::state::AppState>),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[Method::POST],
        "/inbox/lanes/{id}/pause",
        "Pause an inbox lane: its events wait until it is resumed.",
        post(cctui_server_sessions::routes::inbox::pause_lane::<crate::state::AppState>),
        Authn::Bearer,
        ScopeAz(auth::Scope::Dispatch),
    )
    .add(
        &[Method::POST],
        "/inbox/lanes/{id}/resume",
        "Resume a paused inbox lane and deliver what waited.",
        post(cctui_server_sessions::routes::inbox::resume_lane::<crate::state::AppState>),
        Authn::Bearer,
        ScopeAz(auth::Scope::Dispatch),
    )
    .add(
        &[Method::POST],
        "/inbox/events/{id}/retry",
        "Return a dead inbox event to pending with its attempts reset.",
        post(cctui_server_sessions::routes::inbox::retry::<crate::state::AppState>),
        Authn::Bearer,
        ScopeAz(auth::Scope::Dispatch),
    )
}
