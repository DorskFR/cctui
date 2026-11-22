//! Stored key routes.

use super::GET;
use crate::authz::Routes;
use axum::http::Method;
use axum::routing::{delete, get};
use cctui_server_kernel::authz::Authn;
use cctui_server_kernel::authz::Authz::Authenticated;

pub(super) fn register(r: Routes) -> Routes {
    r
        // Provider keys: owner_filter() filter in the handler.
        .add(
            &[GET, Method::POST],
            "/keys",
            "List your provider API keys, or store a new one.",
            get(cctui_server_accounts::routes::credentials::list_api_keys::<crate::state::AppState>)
                .post(cctui_server_accounts::routes::credentials::create_api_key::<crate::state::AppState>),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[Method::DELETE],
            "/keys/{id}",
            "Delete a stored provider API key.",
            delete(cctui_server_accounts::routes::credentials::delete_api_key::<crate::state::AppState>),
            Authn::Bearer,
            Authenticated,
        )
}
