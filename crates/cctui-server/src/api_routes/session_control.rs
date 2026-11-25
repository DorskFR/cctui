//! Per-session control routes: launch, model, account and bindings.

use super::{GET, sess_read, sess_write};
use crate::authz::Routes;
use axum::extract::DefaultBodyLimit;
use axum::http::Method;
use axum::routing::{get, post, put};
use cctui_server_kernel::authz::Authn;

pub(super) fn register(r: Routes) -> Routes {
    r
        // Draft sessions: launch promotes a draft to a live spawn
        // (env entered fresh in the body), discard deletes the draft row.
        .add(
            &[Method::POST],
            "/sessions/{id}/launch",
            "Launch a draft session into a live spawn.",
            post(cctui_server_sessions::routes::spawn::launch_draft::<crate::state::AppState>),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/discard",
            "Discard a draft session.",
            post(cctui_server_sessions::routes::spawn::discard_draft::<crate::state::AppState>),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/schedule-launch",
            "Queue a draft session to launch later.",
            post(cctui_server_sessions::routes::spawn::schedule_draft_launch::<crate::state::AppState>),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/schedule-repeat",
            "Turn a draft session into a repeating schedule.",
            post(cctui_server_sessions::routes::presets::repeat_draft::<crate::state::AppState>),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/cancel-launch",
            "Cancel a draft session's queued launch.",
            post(cctui_server_sessions::routes::spawn::cancel_draft_launch::<crate::state::AppState>),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::PUT],
            "/sessions/{id}/draft",
            "Replace a draft session's stored spawn payload in place.",
            put(cctui_server_sessions::routes::sessions::update_draft::<crate::state::AppState>),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::PUT],
            "/sessions/{id}/draft-attachments",
            "Replace a draft session's stored attachments.",
            put(cctui_server_sessions::routes::spawn::put_draft_attachments::<crate::state::AppState>)
                .layer(DefaultBodyLimit::max(24 * 1024 * 1024)),
            Authn::Bearer,
            sess_write(),
        )
}

pub(super) fn register_control(r: Routes) -> Routes {
    r.add(
        &[Method::POST],
        "/sessions/{id}/interrupt",
        "Interrupt a session's current turn.",
        post(cctui_server_sessions::routes::sessions::interrupt_session::<crate::state::AppState>),
        Authn::Bearer,
        sess_write(),
    )
    .add(
        &[GET],
        "/sessions/{id}/guard",
        "The session's mirrored workflow-guard step, if the pod reported one.",
        get(cctui_server_sessions::guard_step::guard_step),
        Authn::Bearer,
        sess_read(),
    )
    .add(
        &[Method::POST],
        "/sessions/{id}/guard/transition",
        "Ask the session's workflow guard to move to a step.",
        post(cctui_server_sessions::guard_step::guard_transition::<crate::state::AppState>),
        Authn::Bearer,
        sess_write(),
    )
    .add(
        &[Method::POST],
        "/sessions/{id}/resume",
        "Resume an exited session.",
        post(cctui_server_sessions::routes::sessions::resume_session::<crate::state::AppState>),
        Authn::Bearer,
        sess_write(),
    )
    .add(
        &[Method::POST],
        "/sessions/{id}/set-model",
        "Change a session's model.",
        post(cctui_server_sessions::routes::sessions::set_model::<crate::state::AppState>),
        Authn::Bearer,
        sess_write(),
    )
    .add(
        &[Method::POST],
        "/sessions/{id}/switch-account",
        "Switch the account backing a session.",
        post(cctui_server_sessions::routes::sessions::switch_account::<crate::state::AppState>),
        Authn::Bearer,
        sess_write(),
    )
    .add(
        &[GET],
        "/sessions/{id}/bindings",
        "List a session's per-family account bindings.",
        get(cctui_server_sessions::routes::sessions::session_bindings::<crate::state::AppState>),
        Authn::Bearer,
        sess_read(),
    )
}
