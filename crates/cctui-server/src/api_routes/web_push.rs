use axum::http::Method;
use axum::routing::get;

use crate::authz::Routes;
use cctui_server_kernel::authz::Authn;
use cctui_server_kernel::authz::Authz::Human;

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[Method::GET],
        "/notifications/vapid-key",
        "The public VAPID key a browser subscribes to Web Push with.",
        get(cctui_server_notify::routes::web_push::vapid_key::<crate::state::AppState>),
        Authn::Bearer,
        Human,
    )
}
