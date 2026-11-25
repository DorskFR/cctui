use axum::http::Method;
use axum::routing::{get, post};

use super::{sess_read, sess_write};
use crate::authz::Routes;
use cctui_server_kernel::authz::Authn;

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[Method::GET],
        "/sessions/{id}/user-actions",
        "What the agent is waiting on from the user.",
        get(cctui_server_notify::routes::user_actions::get_list::<crate::state::AppState>),
        Authn::Bearer,
        sess_read(),
    )
    .add(
        &[Method::POST],
        "/sessions/{id}/user-actions/{aid}/tick",
        "Resolve one user action as the user (done or dropped).",
        post(cctui_server_notify::routes::user_actions::ui_tick::<crate::state::AppState>),
        Authn::Bearer,
        sess_write(),
    )
}
