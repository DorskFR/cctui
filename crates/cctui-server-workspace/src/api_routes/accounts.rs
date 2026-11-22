//! Account, OAuth and redirect routes.

use super::GET;
use crate::authz::Authz::{Authenticated, Human};
use crate::authz::{Authn, Routes};
use crate::routes;
use axum::http::Method;
use axum::routing::{get, post};

pub(super) fn register(r: Routes) -> Routes {
    r
        // Accounts: `Human` route policy + owner_filter()/resolve_owner in handler.
        .add(
            &[GET],
            "/accounts/settings-catalog",
            "The per-account settings catalog (exposable keys, env allowlist, preset).",
            get(routes::accounts::settings_catalog),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET, Method::POST],
            "/accounts",
            "List your accounts (identities + provider credentials), or create one.",
            get(routes::accounts::list_accounts::<crate::state::AppState>)
                .post(routes::accounts::create_account::<crate::state::AppState>),
            Authn::Bearer,
            Human,
        )
        .add(
            &[Method::POST],
            "/accounts/oauth/start",
            "Begin an OAuth account authorization flow.",
            post(routes::accounts::oauth_start::<crate::state::AppState>),
            Authn::Bearer,
            Human,
        )
        .add(
            &[Method::POST],
            "/accounts/oauth/finish",
            "Complete an OAuth account authorization flow.",
            post(routes::accounts::oauth_finish::<crate::state::AppState>),
            Authn::Bearer,
            Human,
        )
        .add(
            &[GET, Method::PATCH, Method::DELETE],
            "/accounts/{id}",
            "Get, rename/re-env, or delete an account identity.",
            get(routes::accounts::get_account::<crate::state::AppState>)
                .patch(routes::accounts::update_account::<crate::state::AppState>)
                .delete(routes::accounts::delete_account::<crate::state::AppState>),
            Authn::Bearer,
            Human,
        )
        .add(
            &[Method::PUT],
            "/accounts/{id}/redirect",
            "Create/overwrite a launch-time redirect rule for this account.",
            axum::routing::put(routes::account_redirects::put_redirect::<crate::state::AppState>),
            Authn::Bearer,
            Human,
        )
        .add(
            &[GET, Method::PUT],
            "/accounts/{id}/tool-policy",
            "Get or replace the account's gateway tool-call policy.",
            get(routes::tool_policy::get_tool_policy::<crate::state::AppState>)
                .put(routes::tool_policy::put_tool_policy::<crate::state::AppState>),
            Authn::Bearer,
            Human,
        )
        .add(
            &[GET, Method::POST],
            "/accounts/{id}/service-tokens",
            "List or mint the account's service tokens.",
            get(routes::service_tokens::list_service_tokens::<crate::state::AppState>)
                .post(routes::service_tokens::mint_service_token::<crate::state::AppState>),
            Authn::Bearer,
            Human,
        )
        .add(
            &[Method::DELETE],
            "/accounts/{id}/service-tokens/{token_id}",
            "Revoke a service token.",
            axum::routing::delete(
                routes::service_tokens::revoke_service_token::<crate::state::AppState>,
            ),
            Authn::Bearer,
            Human,
        )
        .add(
            &[GET],
            "/redirects",
            "The caller's live account/model redirect rules.",
            get(routes::account_redirects::list_redirects::<crate::state::AppState>),
            Authn::Bearer,
            Human,
        )
        .add(
            &[Method::DELETE],
            "/redirects/{id}",
            "Delete a redirect rule.",
            axum::routing::delete(
                routes::account_redirects::delete_redirect::<crate::state::AppState>,
            ),
            Authn::Bearer,
            Human,
        )
}
