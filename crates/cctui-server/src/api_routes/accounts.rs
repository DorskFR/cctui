//! Account, OAuth and redirect routes.

use super::GET;
use crate::authz::Routes;
use axum::http::Method;
use axum::routing::{get, post};
use cctui_server_kernel::authz::Authn;
use cctui_server_kernel::authz::Authz::{Authenticated, Human};

pub(super) fn register(r: Routes) -> Routes {
    r
        // Accounts: `Human` route policy + owner_filter()/resolve_owner in handler.
        .add(
            &[GET],
            "/accounts/settings-catalog",
            "The per-account settings catalog (exposable keys, env allowlist, preset).",
            get(cctui_server_accounts::routes::accounts::settings_catalog),
            Authn::Bearer,
            Authenticated,
        )
        .add(
            &[GET, Method::POST],
            "/accounts",
            "List your accounts (identities + provider credentials), or create one.",
            get(cctui_server_accounts::routes::accounts::list_accounts::<crate::state::AppState>)
                .post(cctui_server_accounts::routes::accounts::create_account::<crate::state::AppState>),
            Authn::Bearer,
            Human,
        )
        .add(
            &[Method::POST],
            "/accounts/oauth/start",
            "Begin an OAuth account authorization flow.",
            post(cctui_server_accounts::routes::accounts::oauth_start::<crate::state::AppState>),
            Authn::Bearer,
            Human,
        )
        .add(
            &[Method::POST],
            "/accounts/oauth/finish",
            "Complete an OAuth account authorization flow.",
            post(cctui_server_accounts::routes::accounts::oauth_finish::<crate::state::AppState>),
            Authn::Bearer,
            Human,
        )
        .add(
            &[GET, Method::PATCH, Method::DELETE],
            "/accounts/{id}",
            "Get, rename/re-env, or delete an account identity.",
            get(cctui_server_accounts::routes::accounts::get_account::<crate::state::AppState>)
                .patch(cctui_server_accounts::routes::accounts::update_account::<crate::state::AppState>)
                .delete(cctui_server_accounts::routes::accounts::delete_account::<crate::state::AppState>),
            Authn::Bearer,
            Human,
        )
        .add(
            &[Method::PUT],
            "/accounts/{id}/redirect",
            "Create/overwrite a launch-time redirect rule for this account.",
            axum::routing::put(cctui_server_accounts::routes::account_redirects::put_redirect::<crate::state::AppState>),
            Authn::Bearer,
            Human,
        )
        .add(
            &[GET, Method::PUT],
            "/accounts/{id}/tool-policy",
            "Get or replace the account's gateway tool-call policy.",
            get(cctui_server_accounts::routes::tool_policy::get_tool_policy::<crate::state::AppState>)
                .put(cctui_server_accounts::routes::tool_policy::put_tool_policy::<crate::state::AppState>),
            Authn::Bearer,
            Human,
        )
        .add(
            &[GET, Method::POST],
            "/accounts/{id}/service-tokens",
            "List or mint the account's service tokens.",
            get(cctui_server_accounts::routes::service_tokens::list_service_tokens::<crate::state::AppState>)
                .post(cctui_server_accounts::routes::service_tokens::mint_service_token::<crate::state::AppState>),
            Authn::Bearer,
            Human,
        )
        .add(
            &[Method::DELETE],
            "/accounts/{id}/service-tokens/{token_id}",
            "Revoke a service token.",
            axum::routing::delete(
                cctui_server_accounts::routes::service_tokens::revoke_service_token::<crate::state::AppState>,
            ),
            Authn::Bearer,
            Human,
        )
        .add(
            &[GET],
            "/redirects",
            "The caller's live account/model redirect rules.",
            get(cctui_server_accounts::routes::account_redirects::list_redirects::<crate::state::AppState>),
            Authn::Bearer,
            Human,
        )
        .add(
            &[Method::DELETE],
            "/redirects/{id}",
            "Delete a redirect rule.",
            axum::routing::delete(
                cctui_server_accounts::routes::account_redirects::delete_redirect::<crate::state::AppState>,
            ),
            Authn::Bearer,
            Human,
        )
}
