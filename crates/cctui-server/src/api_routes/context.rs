//! Reusable session context: rules, memory notes and prompt templates.

use super::{GET, sess_read};
use crate::authz::Authz::Human;
use crate::authz::{Authn, Routes};
use crate::routes;
use axum::http::Method;
use axum::routing::{get, patch, post};

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[GET, Method::POST],
        "/context",
        "List the caller's context items, or create one.",
        get(routes::context::list_items).post(routes::context::create_item),
        Authn::Bearer,
        Human,
    )
    .add(
        &[GET],
        "/context/resolve",
        "The rules and memories a spawn into these coordinates would carry.",
        get(routes::context::resolve_items),
        Authn::Bearer,
        Human,
    )
    .add(
        &[Method::PATCH, Method::DELETE],
        "/context/{id}",
        "Edit or delete a context item.",
        patch(routes::context::update_item).delete(routes::context::delete_item),
        Authn::Bearer,
        Human,
    )
    .add(
        &[Method::POST],
        "/context/{id}/pending/approve",
        "Make an agent's proposed change to a context item live.",
        post(routes::context::approve_pending),
        Authn::Bearer,
        Human,
    )
    .add(
        &[Method::POST],
        "/context/{id}/pending/reject",
        "Drop an agent's proposed change to a context item.",
        post(routes::context::reject_pending),
        Authn::Bearer,
        Human,
    )
    .add(
        &[GET],
        "/sessions/{id}/context",
        "The rules and memories a session was given.",
        get(routes::context::session_context),
        Authn::Bearer,
        sess_read(),
    )
    .add(
        &[Method::POST],
        "/sessions/{id}/context",
        "Attach memories to a live session (owner only).",
        post(routes::context::attach_to_session),
        Authn::Bearer,
        Human,
    )
}
