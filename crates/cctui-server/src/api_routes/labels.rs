//! Session label routes.

use super::{GET, sess_write};
use crate::authz::Authz::Authenticated;
use crate::authz::{Authn, Routes};
use crate::routes;
use axum::http::Method;
use axum::routing::{delete, get, patch, post};

pub(super) fn register(r: Routes) -> Routes {
    r
        // Session labels: global label definitions (no owner) +
        // per-session attach/detach (authorize_session in the handler).
        .add(
            &[GET, Method::POST],
            "/labels",
            "List label definitions, or create one.",
            get(routes::labels::list_labels).post(routes::labels::create_label),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::PATCH, Method::DELETE],
            "/labels/{id}",
            "Rename or delete a label definition.",
            axum::routing::patch(routes::labels::update_label).delete(routes::labels::delete_label),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/labels",
            "Attach a label to a session.",
            post(routes::labels::attach_label),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::DELETE],
            "/sessions/{id}/labels/{label_id}",
            "Detach a label from a session.",
            axum::routing::delete(routes::labels::detach_label),
            Authn::Bearer,
            sess_write(),
        )
}
