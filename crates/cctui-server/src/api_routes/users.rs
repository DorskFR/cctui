//! User token, ACL and API key routes.

use super::GET;
use crate::authz::Authz::{Authenticated, Scope as ScopeAz};
use crate::authz::{Authn, Routes};
use crate::{auth, routes};
use axum::http::Method;
use axum::routing::{delete, get, patch, post};

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[Method::POST],
        "/users/{id}/tokens",
        "Mint a token for a user (self or admin).",
        post(routes::daemon::mint_user_token),
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
        get(routes::admin_auth::get_user_acls).patch(routes::admin_auth::set_user_acls),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
    .add(
        &[GET, Method::POST],
        "/users/{id}/keys",
        "List or mint a user's API keys (admin).",
        get(routes::admin_auth::list_user_keys).post(routes::admin_auth::mint_user_key),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
    .add(
        &[Method::DELETE],
        "/users/{id}/keys/{kid}",
        "Revoke a user's API key (admin).",
        delete(routes::admin_auth::revoke_user_key),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
    .add(
        &[Method::PATCH],
        "/users/{id}/keys/{kid}/acls",
        "Set a key's scope grant (admin).",
        patch(routes::admin_auth::set_key_acls),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
}
