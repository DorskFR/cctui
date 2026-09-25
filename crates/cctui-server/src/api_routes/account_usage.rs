//! Account provider, usage and limit routes.

use super::GET;
use crate::authz::Authz::Human;
use crate::authz::{Authn, Routes};
use crate::routes;
use axum::http::Method;
use axum::routing::{delete, get, patch, post};

pub(super) fn register(r: Routes) -> Routes {
    r
        // Provider credentials under an account identity: owner-scoped
        // in the handlers like the other account routes.
        .add(
            &[Method::POST],
            "/accounts/{id}/providers",
            "Attach a provider credential to an account.",
            post(routes::accounts::add_provider),
            Authn::Bearer,
            Human,
        )
        .add(
            &[Method::PATCH, Method::DELETE],
            "/accounts/{id}/providers/{provider_id}",
            "Edit or remove one of an account's provider credentials.",
            patch(routes::accounts::update_provider).delete(routes::accounts::delete_provider),
            Authn::Bearer,
            Human,
        )
        .add(
            &[Method::POST],
            "/accounts/{id}/providers/{provider_id}/move",
            "Move a provider credential to another account of the same owner.",
            post(routes::accounts::move_provider),
            Authn::Bearer,
            Human,
        )
        .add(
            &[GET],
            "/accounts/usage",
            "Usage windows of every provider credential the caller owns, in one call.",
            get(routes::accounts::all_accounts_usage),
            Authn::Bearer,
            Human,
        )
        .add(
            &[GET],
            "/accounts/{id}/usage",
            "Get an account's usage/limits.",
            get(routes::accounts::account_usage),
            Authn::Bearer,
            Human,
        )
        .add(
            &[GET],
            "/accounts/{id}/usage/history",
            "Sampled usage of one provider credential over time.",
            get(routes::usage_history::account_usage_history),
            Authn::Bearer,
            Human,
        )
        .add(
            &[GET],
            "/accounts/{id}/usage/closes",
            "Closed usage windows of one provider credential, with unused share.",
            get(routes::usage_history::account_usage_closes),
            Authn::Bearer,
            Human,
        )
        .add(
            &[GET],
            "/accounts/usage/closes",
            "Closed usage windows of every owned credential, with unused share.",
            get(routes::usage_history::all_usage_closes),
            Authn::Bearer,
            Human,
        )
        .add(
            &[Method::POST],
            "/accounts/{id}/limit-reset",
            "Claim a usage-limit reset on a provider credential.",
            post(routes::limit_reset::limit_reset),
            Authn::Bearer,
            Human,
        )
}
