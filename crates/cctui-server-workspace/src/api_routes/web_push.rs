use axum::http::Method;
use axum::routing::get;

use crate::authz::Authz::Human;
use crate::authz::{Authn, Routes};
use crate::routes;

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[Method::GET],
        "/notifications/vapid-key",
        "The public VAPID key a browser subscribes to Web Push with.",
        get(routes::web_push::vapid_key),
        Authn::Bearer,
        Human,
    )
}
