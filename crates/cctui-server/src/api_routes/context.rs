//! Reusable session context: rules, memory notes and prompt templates.

use super::{GET, sess_read, sess_write};
use crate::authz::Routes;
use axum::http::Method;
use axum::routing::{get, patch, post};
use cctui_server_kernel::authz::Authn;
use cctui_server_kernel::authz::Authz::Human;

pub(super) fn register(r: Routes) -> Routes {
    r
        .add(
            &[GET, Method::POST, Method::PATCH],
            "/context",
            "List the caller's context items, create one, or edit the match of many at once.",
            get(cctui_server_sessions::routes::context::list_items::<crate::state::AppState>)
                .post(cctui_server_sessions::routes::context::create_item::<crate::state::AppState>)
                .patch(
                    cctui_server_sessions::routes::context::reach::bulk_update::<
                        crate::state::AppState,
                    >,
                ),
            Authn::Bearer,
            Human,
        )
        .add(
            &[GET],
            "/context/resolve",
            "The rules and memories a spawn into these coordinates would carry.",
            get(cctui_server_sessions::routes::context::resolve_items::<crate::state::AppState>),
            Authn::Bearer,
            Human,
        )
        .add(
            &[Method::PATCH, Method::DELETE],
            "/context/{id}",
            "Edit or delete a context item.",
            patch(cctui_server_sessions::routes::context::update_item::<crate::state::AppState>)
                .delete(
                    cctui_server_sessions::routes::context::delete_item::<crate::state::AppState>,
                ),
            Authn::Bearer,
            Human,
        )
        .add(
            &[Method::POST],
            "/context/{id}/pending/approve",
            "Make an agent's proposed change to a context item live.",
            post(cctui_server_sessions::routes::context::approve_pending::<crate::state::AppState>),
            Authn::Bearer,
            Human,
        )
        .add(
            &[Method::POST],
            "/context/{id}/pending/reject",
            "Drop an agent's proposed change to a context item.",
            post(cctui_server_sessions::routes::context::reject_pending::<crate::state::AppState>),
            Authn::Bearer,
            Human,
        )
        .add(
            &[GET],
            "/sessions/{id}/context",
            "The rules and memories a session was given.",
            get(cctui_server_sessions::routes::context::session_context::<crate::state::AppState>),
            Authn::Bearer,
            sess_read(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/context",
            "Attach memories to a live session (owner only).",
            post(
                cctui_server_sessions::routes::context::attach_to_session::<crate::state::AppState>,
            ),
            Authn::Bearer,
            sess_write(),
        )
}
