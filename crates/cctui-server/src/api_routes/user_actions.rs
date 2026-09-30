use axum::http::Method;
use axum::routing::{get, post};

use super::{sess_read, sess_write};
use crate::authz::{Authn, Routes};
use crate::routes;

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[Method::GET],
        "/sessions/{id}/user-actions",
        "What the agent is waiting on from the user (owner only).",
        get(routes::user_actions::get_list),
        Authn::Bearer,
        sess_read(),
    )
    .add(
        &[Method::POST],
        "/sessions/{id}/user-actions/{aid}/tick",
        "Resolve one user action as the user (done or dropped).",
        post(routes::user_actions::ui_tick),
        Authn::Bearer,
        sess_write(),
    )
}
