//! Reusable session context: memory notes and prompt templates.

use super::GET;
use crate::authz::Authz::Human;
use crate::authz::{Authn, Routes};
use crate::routes;
use axum::http::Method;
use axum::routing::{get, patch};

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
        "The context a spawn into these coordinates would attach on its own.",
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
}
