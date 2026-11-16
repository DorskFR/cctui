use axum::http::Method;
use axum::routing::get;

use crate::authz::Authz::Authenticated;
use crate::authz::{Authn, Routes};
use crate::routes;

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[Method::GET, Method::POST],
        "/presets",
        "List your launch presets, or save one.",
        get(routes::presets::list).post(routes::presets::create),
        Authn::Bearer,
        Authenticated,
    )
    .add(
        &[Method::GET, Method::PUT, Method::DELETE],
        "/presets/{id}",
        "Read, replace or delete one launch preset.",
        get(routes::presets::get_one).put(routes::presets::update).delete(routes::presets::delete),
        Authn::Bearer,
        Authenticated,
    )
}
