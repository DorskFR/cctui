//! Per-session brief, thread, fork, archive, pin, keepalive and policy routes.

use super::{sess_read, sess_write};
use crate::authz::Routes;
use axum::http::Method;
use axum::routing::{get, patch, post};
use cctui_server_kernel::authz::Authn;

pub(super) fn register(r: Routes) -> Routes {
    let r = r
        .add(
            &[Method::GET],
            "/sessions/{id}/brief",
            "Render a session's user/assistant transcript as a capped markdown brief.",
            get(cctui_server_sessions::brief::session_brief::<crate::state::AppState>),
            Authn::Bearer,
            sess_read(),
        )
        .add(
            &[Method::GET],
            "/sessions/{id}/thread",
            "List the follow-up thread a session belongs to, oldest first.",
            get(cctui_server_sessions::followup::session_thread::<crate::state::AppState>),
            Authn::Bearer,
            sess_read(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/fork",
            "Fork a session into a new one.",
            post(cctui_server_sessions::routes::sessions::fork_session::<crate::state::AppState>),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/auto-approve",
            "Toggle auto-approval of tool-use for a session.",
            post(cctui_server_sessions::routes::sessions::set_auto_approve::<crate::state::AppState>),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/archive",
            "Archive a single session.",
            post(cctui_server_sessions::routes::sessions::archive_session::<crate::state::AppState>),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/unarchive",
            "Unarchive a single session.",
            post(cctui_server_sessions::routes::sessions::unarchive_session::<crate::state::AppState>),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/pin",
            "Pin a single session.",
            post(cctui_server_sessions::routes::sessions::pin_session::<crate::state::AppState>),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/unpin",
            "Unpin a single session.",
            post(cctui_server_sessions::routes::sessions::unpin_session::<crate::state::AppState>),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/keepalive",
            "Set or clear the session's prompt-cache keep-alive schedule.",
            post(cctui_server_sessions::routes::sessions::set_keepalive::<crate::state::AppState>),
            Authn::Bearer,
            sess_write(),
        );
    register_policies(r)
}

fn register_policies(r: Routes) -> Routes {
    r.add(
        &[Method::GET],
        "/sessions/{id}/hook-policy",
        "Read the hook rules the session was launched with.",
        get(cctui_server_accounts::routes::tool_policy::get_session_hook_policy::<
            crate::state::AppState,
        >),
        Authn::Bearer,
        sess_read(),
    )
    .add(
        &[Method::GET, Method::PUT],
        "/sessions/{id}/tool-guard",
        "Read or toggle the session's gateway tool-guard bypass.",
        get(cctui_server_accounts::routes::tool_guard::get_tool_guard::<crate::state::AppState>)
            .put(
                cctui_server_accounts::routes::tool_guard::put_tool_guard::<crate::state::AppState>,
            ),
        Authn::Bearer,
        sess_write(),
    )
    .add(
        &[Method::PATCH],
        "/sessions/{id}/plugins/{plugin_id}",
        "Set or clear a session's per-plugin data slot.",
        patch(
            cctui_server_extras::routes::session_plugins::set_session_plugin::<
                crate::state::AppState,
            >,
        ),
        Authn::Bearer,
        sess_write(),
    )
}
