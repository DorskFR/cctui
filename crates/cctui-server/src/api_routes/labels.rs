//! Session label routes.

use super::{GET, sess_write};
use crate::authz::Routes;
use axum::http::Method;
use axum::routing::{get, post};
use cctui_server_kernel::authz::Authn;
use cctui_server_kernel::authz::Authz::Authenticated;

pub(super) fn register(r: Routes) -> Routes {
    r
        // Session labels: global label definitions (no owner) +
        // per-session attach/detach (authorize_session in the handler).
        .add(
            &[GET, Method::POST],
            "/labels",
            "List label definitions, or create one.",
            get(cctui_server_sessions::routes::labels::list_labels::<crate::state::AppState>).post(
                cctui_server_sessions::routes::labels::create_label::<crate::state::AppState>,
            ),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::PATCH, Method::DELETE],
            "/labels/{id}",
            "Rename or delete a label definition.",
            axum::routing::patch(
                cctui_server_sessions::routes::labels::update_label::<crate::state::AppState>,
            )
            .delete(cctui_server_sessions::routes::labels::delete_label::<crate::state::AppState>),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/labels",
            "Attach a label to a session.",
            post(cctui_server_sessions::routes::labels::attach_label::<crate::state::AppState>),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::DELETE],
            "/sessions/{id}/labels/{label_id}",
            "Detach a label from a session.",
            axum::routing::delete(
                cctui_server_sessions::routes::labels::detach_label::<crate::state::AppState>,
            ),
            Authn::Bearer,
            sess_write(),
        )
}
