//! Admin user and machine management routes.

use super::GET;
use crate::authz::Authz::Scope as ScopeAz;
use crate::authz::{Authn, Routes};
use crate::{auth, routes};
use axum::http::Method;
use axum::routing::{delete, get, patch, post};

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[Method::DELETE, Method::PATCH],
        "/admin/users/{id}",
        "Revoke or update a user (admin).",
        delete(routes::admin_auth::revoke_user).patch(routes::admin_auth::update_user),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
    .add(
        &[Method::DELETE],
        "/admin/users/{id}/purge",
        "Hard-delete a user and all their data (admin).",
        delete(routes::admin_auth::purge_user),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
    .add(
        &[Method::POST],
        "/admin/users/{id}/rotate",
        "Rotate a user's tokens (admin).",
        post(routes::admin_auth::rotate_user),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
    .add(
        &[GET],
        "/admin/users/{id}/machines",
        "List a user's machines (admin).",
        get(routes::admin_auth::list_user_machines),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
    .add(
        &[GET],
        "/admin/users/{id}/tokens",
        "List a user's tokens (admin).",
        get(routes::admin_auth::list_user_tokens),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
    .add(
        &[Method::PATCH, Method::DELETE],
        "/admin/users/{id}/tokens/{token_id}",
        "Relabel or revoke a user's token (admin).",
        patch(routes::admin_auth::relabel_user_token).delete(routes::admin_auth::revoke_user_token),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
    .add(
        &[Method::DELETE],
        "/admin/users/{id}/tokens/{token_id}/purge",
        "Hard-delete a user's token (admin).",
        delete(routes::admin_auth::delete_user_token),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
    .add(
        &[Method::DELETE, Method::PATCH],
        "/admin/machines/{id}",
        "Revoke or rename a machine (admin).",
        delete(routes::admin_auth::revoke_machine).patch(routes::admin_auth::rename_machine),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
    .add(
        &[Method::POST],
        "/admin/machines/{id}/rotate",
        "Rotate a machine's key (admin).",
        post(routes::admin_auth::rotate_machine),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
    .add(
        &[Method::DELETE],
        "/admin/machines/{id}/purge",
        "Hard-delete a machine (admin).",
        delete(routes::admin_auth::delete_machine),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
}
