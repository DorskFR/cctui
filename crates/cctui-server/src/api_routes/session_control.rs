//! Per-session control routes: launch, model, account and bindings.

use super::{GET, sess_read, sess_write};
use crate::authz::{Authn, Routes};
use crate::routes;
use axum::http::Method;
use axum::routing::{get, post, put};

pub(super) fn register(r: Routes) -> Routes {
    r
        // Draft sessions: launch promotes a draft to a live spawn
        // (env entered fresh in the body), discard deletes the draft row.
        .add(
            &[Method::POST],
            "/sessions/{id}/launch",
            "Launch a draft session into a live spawn.",
            post(routes::spawn::launch_draft),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/discard",
            "Discard a draft session.",
            post(routes::spawn::discard_draft),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::PUT],
            "/sessions/{id}/draft",
            "Replace a draft session's stored spawn payload in place.",
            put(routes::sessions::update_draft),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/interrupt",
            "Interrupt a session's current turn.",
            post(routes::sessions::interrupt_session),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/resume",
            "Resume an exited session.",
            post(routes::sessions::resume_session),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/set-model",
            "Change a session's model.",
            post(routes::sessions::set_model),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[Method::POST],
            "/sessions/{id}/switch-account",
            "Switch the account backing a session.",
            post(routes::sessions::switch_account),
            Authn::Bearer,
            sess_write(),
        )
        .add(
            &[GET],
            "/sessions/{id}/bindings",
            "List a session's per-family account bindings.",
            get(routes::sessions::session_bindings),
            Authn::Bearer,
            sess_read(),
        )
}
