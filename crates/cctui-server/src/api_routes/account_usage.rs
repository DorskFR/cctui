//! Account provider, usage and limit routes.

use super::GET;
use crate::authz::Routes;
use axum::http::Method;
use axum::routing::{get, patch, post};
use cctui_server_kernel::authz::Authn;
use cctui_server_kernel::authz::Authz::Human;

pub(super) fn register(r: Routes) -> Routes {
    r
        // Provider credentials under an account identity: owner-scoped
        // in the handlers like the other account routes.
        .add(
            &[Method::POST],
            "/accounts/{id}/providers",
            "Attach a provider credential to an account.",
            post(cctui_server_accounts::routes::accounts::add_provider::<crate::state::AppState>),
            Authn::Bearer,
            Human,
        )
        .add(
            &[Method::PATCH, Method::DELETE],
            "/accounts/{id}/providers/{provider_id}",
            "Edit or remove one of an account's provider credentials.",
            patch(
                cctui_server_accounts::routes::accounts::update_provider::<crate::state::AppState>,
            )
            .delete(
                cctui_server_accounts::routes::accounts::delete_provider::<crate::state::AppState>,
            ),
            Authn::Bearer,
            Human,
        )
        .add(
            &[Method::POST],
            "/accounts/{id}/providers/{provider_id}/move",
            "Move a provider credential to another account of the same owner.",
            post(cctui_server_accounts::routes::accounts::move_provider::<crate::state::AppState>),
            Authn::Bearer,
            Human,
        )
        .add(
            &[GET],
            "/accounts/usage",
            "Usage windows of every provider credential the caller owns, in one call.",
            get(cctui_server_accounts::routes::accounts::all_accounts_usage::<
                crate::state::AppState,
            >),
            Authn::Bearer,
            Human,
        )
        .add(
            &[GET],
            "/accounts/{id}/usage",
            "Get an account's usage/limits.",
            get(cctui_server_accounts::routes::accounts::account_usage::<crate::state::AppState>),
            Authn::Bearer,
            Human,
        )
        .add(
            &[GET],
            "/accounts/{id}/usage/history",
            "Sampled usage of one provider credential over time.",
            get(cctui_server_accounts::routes::usage_history::account_usage_history::<
                crate::state::AppState,
            >),
            Authn::Bearer,
            Human,
        )
        .add(
            &[GET],
            "/accounts/{id}/usage/closes",
            "Closed usage windows of one provider credential, with unused share.",
            get(cctui_server_accounts::routes::usage_history::account_usage_closes::<
                crate::state::AppState,
            >),
            Authn::Bearer,
            Human,
        )
        .add(
            &[GET],
            "/accounts/usage/closes",
            "Closed usage windows of every owned credential, with unused share.",
            get(cctui_server_accounts::routes::usage_history::all_usage_closes::<
                crate::state::AppState,
            >),
            Authn::Bearer,
            Human,
        )
        .add(
            &[GET],
            "/accounts/usage/ledger",
            "Every reset of every visible credential's windows, with what went unused.",
            get(cctui_server_accounts::routes::usage_ledger::account_usage_ledger::<
                crate::state::AppState,
            >),
            Authn::Bearer,
            Human,
        )
        .add(
            &[Method::POST],
            "/accounts/{id}/limit-reset",
            "Claim a usage-limit reset on a provider credential.",
            post(cctui_server_accounts::routes::limit_reset::limit_reset::<crate::state::AppState>),
            Authn::Bearer,
            Human,
        )
}
