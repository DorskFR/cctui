//! Dispatch hooks forwarded to the account functions.

#[macro_export]
macro_rules! impl_dispatch_account_resolution {
    ($host:ty) => {
        impl ::cctui_server_dispatch::hooks::DispatchAccountResolution for $host {
            async fn in_flight_by_provider(
                &self,
                providers: &[::uuid::Uuid],
            ) -> ::std::collections::HashMap<::uuid::Uuid, u32> {
                ::cctui_server_accounts::account_resolve::in_flight_by_provider(self, providers)
                    .await
            }
            async fn needing_reauth(
                &self,
                providers: &[::uuid::Uuid],
            ) -> ::std::collections::HashSet<::uuid::Uuid> {
                ::cctui_server_accounts::account_resolve::needing_reauth(self, providers).await
            }
            async fn resolve_account_or_pool(
                &self,
                user_id: ::uuid::Uuid,
                family: ::cctui_server_accounts::gateway::Family,
                model: Option<&str>,
                name: &str,
            ) -> Result<
                ::cctui_server_accounts::account_resolve::Bound,
                ::cctui_server_accounts::account_resolve::ResolveError,
            > {
                ::cctui_server_accounts::account_resolve::resolve_account_or_pool(
                    self, user_id, family, model, name,
                )
                .await
            }
            async fn resolve_pool(
                &self,
                user_id: ::uuid::Uuid,
                family: ::cctui_server_accounts::gateway::Family,
                model: Option<&str>,
                pool_name: &str,
            ) -> Result<
                (String, ::uuid::Uuid),
                ::cctui_server_accounts::account_resolve::ResolveError,
            > {
                ::cctui_server_accounts::account_resolve::resolve_pool(
                    self, user_id, family, model, pool_name,
                )
                .await
            }
            async fn resolve_pool_by_id(
                &self,
                user_id: ::uuid::Uuid,
                family: ::cctui_server_accounts::gateway::Family,
                model: Option<&str>,
                pool_id: ::uuid::Uuid,
            ) -> Result<
                (String, ::uuid::Uuid),
                ::cctui_server_accounts::account_resolve::ResolveError,
            > {
                ::cctui_server_accounts::account_resolve::resolve_pool_by_id(
                    self, user_id, family, model, pool_id,
                )
                .await
            }
            async fn stamp_pool(&self, session_id: &str, pool_id: ::uuid::Uuid) {
                ::cctui_server_accounts::account_resolve::stamp_pool(self, session_id, pool_id)
                    .await;
            }
        }
    };
}

#[macro_export]
macro_rules! impl_dispatch_mint {
    ($host:ty) => {
        impl ::cctui_server_dispatch::hooks::DispatchMint for $host {
            async fn account_provider_rows(
                &self,
                user_id: ::uuid::Uuid,
                account_name: &str,
            ) -> Result<
                Option<Vec<::cctui_server_accounts::gateway::mint::ProviderRow>>,
                ::sqlx::Error,
            > {
                ::cctui_server_accounts::gateway::mint::account_provider_rows(
                    self,
                    user_id,
                    account_name,
                )
                .await
            }
            async fn acting_user(
                &self,
                session_id: &str,
                known: Option<::uuid::Uuid>,
            ) -> Option<::uuid::Uuid> {
                ::cctui_server_accounts::gateway::mint::acting_user(self, session_id, known).await
            }
            async fn flip_model_redirect(
                &self,
                user_id: ::uuid::Uuid,
                provider_row_id: ::uuid::Uuid,
                family: ::cctui_server_accounts::gateway::Family,
                requested: &str,
            ) -> String {
                ::cctui_server_accounts::gateway::mint::flip_model_redirect(
                    self,
                    user_id,
                    provider_row_id,
                    family,
                    requested,
                )
                .await
            }
            async fn mint_session_env(
                &self,
                user_id: ::uuid::Uuid,
                account_name: &str,
                family: ::cctui_server_accounts::gateway::Family,
                session_id: &str,
            ) -> Result<
                ::std::collections::BTreeMap<String, String>,
                ::cctui_server_accounts::gateway::mint::MintSessionEnvError,
            > {
                ::cctui_server_accounts::gateway::mint::mint_session_env(
                    self,
                    user_id,
                    account_name,
                    family,
                    session_id,
                )
                .await
            }
            async fn mint_session_env_all_families(
                &self,
                user_id: ::uuid::Uuid,
                account_name: &str,
                primary: ::cctui_server_accounts::gateway::Family,
                session_id: &str,
            ) -> Result<
                ::std::collections::BTreeMap<String, String>,
                ::cctui_server_accounts::gateway::mint::MintSessionEnvError,
            > {
                ::cctui_server_accounts::gateway::mint::mint_session_env_all_families(
                    self,
                    user_id,
                    account_name,
                    primary,
                    session_id,
                )
                .await
            }
            async fn mint_session_env_for_account(
                &self,
                provider_id: ::uuid::Uuid,
                session_id: &str,
            ) -> Result<Option<::std::collections::BTreeMap<String, String>>, ::sqlx::Error> {
                ::cctui_server_accounts::gateway::mint::mint_session_env_for_account(
                    self,
                    provider_id,
                    session_id,
                )
                .await
            }
            async fn resolve_account_model(
                &self,
                user_id: ::uuid::Uuid,
                account_name: &str,
                family: ::cctui_server_accounts::gateway::Family,
                model: &str,
            ) -> String {
                ::cctui_server_accounts::gateway::mint::resolve_account_model(
                    self,
                    user_id,
                    account_name,
                    family,
                    model,
                )
                .await
            }
            async fn resolve_session_settings(
                &self,
                session_id: &str,
            ) -> Option<::serde_json::Value> {
                ::cctui_server_accounts::gateway::mint::resolve_session_settings(self, session_id)
                    .await
            }
        }
    };
}

#[macro_export]
macro_rules! impl_dispatch_usage {
    ($host:ty) => {
        impl ::cctui_server_dispatch::hooks::DispatchUsage for $host {
            async fn reload_account(
                &self,
                id: ::uuid::Uuid,
            ) -> Option<::cctui_server_accounts::gateway::credentials::Account> {
                ::cctui_server_accounts::gateway::credentials::reload_account(self, id).await
            }
            async fn session_spend_usd(
                &self,
                account_id: ::uuid::Uuid,
                session_id: &str,
            ) -> Option<f64> {
                ::cctui_server_accounts::gateway::usage::session_spend_usd(
                    self, account_id, session_id,
                )
                .await
            }
            async fn usage_for_soft_limit(
                &self,
                account_id: ::uuid::Uuid,
            ) -> Option<::serde_json::Value> {
                ::cctui_server_accounts::gateway::usage::usage_for_soft_limit(self, account_id)
                    .await
            }
        }
    };
}

#[macro_export]
macro_rules! impl_dispatch_account_store {
    ($host:ty, $pool:ident) => {
        impl ::cctui_server_dispatch::hooks::DispatchAccountStore for $host {
            async fn account_rules(
                &self,
                session_id: &str,
            ) -> Result<Vec<::cctui_proto::hook_policy::HookRule>, ::sqlx::Error> {
                ::cctui_server_accounts::hook_policy::account_rules(&self.$pool, session_id).await
            }
            async fn for_child(
                &self,
                parent_session_id: &str,
                additions: Vec<::cctui_proto::hook_policy::HookRule>,
            ) -> Result<
                ::cctui_proto::hook_policy::HookPolicy,
                ::cctui_server_accounts::hook_policy::PolicyError,
            > {
                ::cctui_server_accounts::hook_policy::for_child(
                    &self.$pool,
                    parent_session_id,
                    additions,
                )
                .await
            }
            async fn for_dispatch(
                &self,
                session_id: &str,
                adapter: &str,
                payload: &mut ::serde_json::Value,
            ) -> Result<(), ::cctui_server_accounts::hook_policy::PolicyError> {
                ::cctui_server_accounts::hook_policy::for_dispatch(
                    &self.$pool,
                    session_id,
                    adapter,
                    payload,
                )
                .await
            }
            async fn record_hook_policy(
                &self,
                command: ::uuid::Uuid,
                carried: &::cctui_proto::hook_policy::SessionHookPolicy,
            ) -> Result<(), ::sqlx::Error> {
                ::cctui_server_accounts::hook_policy::record(&self.$pool, command, carried).await
            }
            async fn account_pool_by_name(
                &self,
                user_id: ::uuid::Uuid,
                name: &str,
            ) -> Result<Option<::cctui_proto::api::account_pools::AccountPool>, ::sqlx::Error> {
                ::cctui_server_accounts::store::account_pools::by_name(&self.$pool, user_id, name)
                    .await
            }
            async fn live_for_launch(
                &self,
                user_id: ::uuid::Uuid,
            ) -> Result<Vec<::cctui_proto::api::account_redirects::AccountRedirect>, ::sqlx::Error>
            {
                ::cctui_server_accounts::store::account_redirects::live_for_launch(
                    &self.$pool,
                    user_id,
                )
                .await
            }
        }
    };
}
