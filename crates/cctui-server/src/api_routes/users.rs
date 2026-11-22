//! User token, ACL and API key routes.

use super::GET;
use crate::authz::Routes;
use axum::http::Method;
use axum::routing::{delete, get, patch, post};
use cctui_server_kernel::auth;
use cctui_server_kernel::authz::Authn;
use cctui_server_kernel::authz::Authz::{Authenticated, Scope as ScopeAz};

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[Method::POST],
        "/users/{id}/tokens",
        "Mint a token for a user (self or admin).",
        post(cctui_server_sessions::routes::daemon::mint_user_token::<crate::state::AppState>),
        Authn::Bearer,
        // In-handler: admin may mint for anyone; a user only for itself.
        Authenticated,
    )
    // per-user scope (ceiling) + per-key (grant) management — all
    // admin-only (`forbid_or`).
    .add(
        &[GET, Method::PATCH],
        "/users/{id}/acls",
        "Get or set a user's scope ceiling (admin).",
        get(cctui_server_accounts::identity::admin::get_user_acls::<crate::state::AppState>)
            .patch(cctui_server_accounts::identity::admin::set_user_acls::<crate::state::AppState>),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
    .add(
        &[GET, Method::POST],
        "/users/{id}/keys",
        "List or mint a user's API keys (admin).",
        get(cctui_server_accounts::identity::admin::list_user_keys::<crate::state::AppState>)
            .post(cctui_server_accounts::identity::admin::mint_user_key::<crate::state::AppState>),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
    .add(
        &[Method::DELETE],
        "/users/{id}/keys/{kid}",
        "Revoke a user's API key (admin).",
        delete(cctui_server_accounts::identity::admin::revoke_user_key::<crate::state::AppState>),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
    .add(
        &[Method::PATCH],
        "/users/{id}/keys/{kid}/acls",
        "Set a key's scope grant (admin).",
        patch(cctui_server_accounts::identity::admin::set_key_acls::<crate::state::AppState>),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
}
