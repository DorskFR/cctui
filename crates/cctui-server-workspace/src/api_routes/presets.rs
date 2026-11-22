use axum::extract::DefaultBodyLimit;
use axum::http::Method;
use axum::routing::{get, post, put};

use crate::authz::Authz::Authenticated;
use crate::authz::{Authn, Routes};
use crate::routes;

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[Method::GET, Method::POST],
        "/presets",
        "List your launch presets, or save one.",
        get(routes::presets::list::<crate::state::AppState>)
            .post(routes::presets::create::<crate::state::AppState>),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[Method::GET, Method::PUT, Method::DELETE],
        "/presets/{id}",
        "Read, replace or delete one launch preset.",
        get(routes::presets::get_one::<crate::state::AppState>)
            .put(routes::presets::update::<crate::state::AppState>)
            .delete(routes::presets::delete::<crate::state::AppState>),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[Method::PUT],
        "/presets/{id}/attachments",
        "Replace the files every run of a launch preset gets.",
        put(routes::presets::put_attachments::<crate::state::AppState>)
            .layer(DefaultBodyLimit::max(24 * 1024 * 1024)),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[Method::POST],
        "/presets/{id}/run",
        "Run a launch preset now, whatever its schedule.",
        post(routes::presets::run::<crate::state::AppState>),
        Authn::Bearer,
        Authenticated,
    )
}
