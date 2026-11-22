//! Admin user and machine management routes.

use super::GET;
use crate::auth;
use crate::authz::Authz::Scope as ScopeAz;
use crate::authz::{Authn, Routes};
use axum::http::Method;
use axum::routing::{delete, get, patch, post};

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[Method::DELETE, Method::PATCH],
        "/admin/users/{id}",
        "Revoke or update a user (admin).",
        delete(cctui_server_accounts::identity::admin::revoke_user::<crate::state::AppState>)
            .patch(cctui_server_accounts::identity::admin::update_user::<crate::state::AppState>),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
    .add(
        &[Method::DELETE],
        "/admin/users/{id}/purge",
        "Hard-delete a user and all their data (admin).",
        delete(cctui_server_accounts::identity::admin::purge_user::<crate::state::AppState>),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
    .add(
        &[Method::POST],
        "/admin/users/{id}/rotate",
        "Rotate a user's tokens (admin).",
        post(cctui_server_accounts::identity::admin::rotate_user::<crate::state::AppState>),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
    .add(
        &[GET],
        "/admin/users/{id}/machines",
        "List a user's machines (admin).",
        get(cctui_server_accounts::identity::admin::list_user_machines::<crate::state::AppState>),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
    .add(
        &[GET],
        "/admin/users/{id}/tokens",
        "List a user's tokens (admin).",
        get(cctui_server_accounts::identity::admin::list_user_tokens::<crate::state::AppState>),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
    .add(
        &[Method::PATCH, Method::DELETE],
        "/admin/users/{id}/tokens/{token_id}",
        "Relabel or revoke a user's token (admin).",
        patch(cctui_server_accounts::identity::admin::relabel_user_token::<crate::state::AppState>)
            .delete(
                cctui_server_accounts::identity::admin::revoke_user_token::<crate::state::AppState>,
            ),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
    .add(
        &[Method::DELETE],
        "/admin/users/{id}/tokens/{token_id}/purge",
        "Hard-delete a user's token (admin).",
        delete(cctui_server_accounts::identity::admin::delete_user_token::<crate::state::AppState>),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
    .add(
        &[GET, Method::PUT, Method::DELETE],
        "/admin/users/{id}/dispatch-policy",
        "Get, replace or remove a user's dispatch policy (admin).",
        get(cctui_server_dispatch::routes::dispatch_policy::get_policy::<crate::state::AppState>)
            .put(cctui_server_dispatch::routes::dispatch_policy::set_policy::<crate::state::AppState>)
            .delete(cctui_server_dispatch::routes::dispatch_policy::delete_policy::<crate::state::AppState>),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
    .add(
        &[Method::DELETE, Method::PATCH],
        "/admin/machines/{id}",
        "Revoke or rename a machine (admin).",
        delete(cctui_server_accounts::identity::admin::revoke_machine::<crate::state::AppState>)
            .patch(
                cctui_server_accounts::identity::admin::rename_machine::<crate::state::AppState>,
            ),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
    .add(
        &[Method::POST],
        "/admin/machines/{id}/rotate",
        "Rotate a machine's key (admin).",
        post(cctui_server_accounts::identity::admin::rotate_machine::<crate::state::AppState>),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
    .add(
        &[Method::DELETE],
        "/admin/machines/{id}/purge",
        "Hard-delete a machine (admin).",
        delete(cctui_server_accounts::identity::admin::delete_machine::<crate::state::AppState>),
        Authn::Bearer,
        ScopeAz(auth::Scope::Admin),
    )
}
