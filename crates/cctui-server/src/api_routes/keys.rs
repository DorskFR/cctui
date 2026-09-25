//! Stored key routes.

use super::GET;
use crate::authz::Authz::Authenticated;
use crate::authz::{Authn, Routes};
use crate::routes;
use axum::http::Method;
use axum::routing::{delete, get, post};

pub(super) fn register(r: Routes) -> Routes {
    r
        // Provider keys: owner_filter() filter in the handler.
        .add(
            &[GET, Method::POST],
            "/keys",
            "List your provider API keys, or store a new one.",
            get(routes::credentials::list_api_keys).post(routes::credentials::create_api_key),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::DELETE],
            "/keys/{id}",
            "Delete a stored provider API key.",
            delete(routes::credentials::delete_api_key),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET],
            "/keys/{id}/value",
            "Reveal a stored provider API key's value.",
            get(routes::credentials::get_api_key_value),
            Authn::Bearer,
            Authenticated,
        )
}
