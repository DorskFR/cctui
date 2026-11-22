//! Resource sharing routes.

use super::GET;
use crate::authz::Routes;
use axum::http::Method;
use axum::routing::{delete, get};
use cctui_server_kernel::authz::Authn;
use cctui_server_kernel::authz::Authz::Human;

pub(super) fn register(r: Routes) -> Routes {
    r
        // Account sharing management: owner-scoped in the handler
        // (require_account_owner) just like the other account routes.
        .add(
            &[GET, Method::POST],
            "/accounts/{id}/shares",
            "List or grant shares of an account to other users.",
            get(cctui_server_accounts::routes::accounts::list_shares::<crate::state::AppState>)
                .post(
                    cctui_server_accounts::routes::accounts::grant_share::<crate::state::AppState>,
                ),
            Authn::Bearer,
            Human,
        )
        .add(
            &[Method::DELETE],
            "/accounts/{id}/shares/{user_id}",
            "Revoke a user's share of an account.",
            delete(cctui_server_accounts::routes::accounts::revoke_share::<crate::state::AppState>),
            Authn::Bearer,
            Human,
        )
        // Generic resource-sharing CRUD: owner-scoped in the handler
        // (require_owner) for any shareable kind. The account routes above are
        // static-path back-compat aliases; these serve machine/dispatcher/etc.
        .add(
            &[GET, Method::POST],
            "/{resource_type}/{id}/shares",
            "List or grant shares of a resource to other users.",
            get(cctui_server_sessions::routes::shares::list_shares::<crate::state::AppState>)
                .post(cctui_server_sessions::routes::shares::grant_share::<crate::state::AppState>),
            Authn::Bearer,
            Human,
        )
        .add(
            &[Method::DELETE],
            "/{resource_type}/{id}/shares/{user_id}",
            "Revoke a user's share of a resource.",
            delete(cctui_server_sessions::routes::shares::revoke_share::<crate::state::AppState>),
            Authn::Bearer,
            Human,
        )
}
