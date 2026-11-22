//! Dispatch hooks forwarded to the account functions.

#[macro_export]
macro_rules! impl_dispatch_account_resolution {
    ($host:ty) => {
        impl $crate::hooks::DispatchAccountResolution for $host {
            async fn in_flight_by_provider(
                &self,
                providers: &[::uuid::Uuid],
            ) -> ::std::collections::HashMap<::uuid::Uuid, u32> {
                $crate::account_resolve::in_flight_by_provider(self, providers).await
            }
            async fn needing_reauth(
                &self,
                providers: &[::uuid::Uuid],
            ) -> ::std::collections::HashSet<::uuid::Uuid> {
                $crate::account_resolve::needing_reauth(self, providers).await
            }
            async fn resolve_account_or_pool(
                &self,
                user_id: ::uuid::Uuid,
                family: $crate::gateway::Family,
                model: Option<&str>,
                name: &str,
            ) -> Result<$crate::account_resolve::Bound, $crate::account_resolve::ResolveError> {
                $crate::account_resolve::resolve_account_or_pool(self, user_id, family, model, name)
                    .await
            }
            async fn resolve_pool(
                &self,
                user_id: ::uuid::Uuid,
                family: $crate::gateway::Family,
                model: Option<&str>,
                pool_name: &str,
            ) -> Result<(String, ::uuid::Uuid), $crate::account_resolve::ResolveError> {
                $crate::account_resolve::resolve_pool(self, user_id, family, model, pool_name).await
            }
            async fn resolve_pool_by_id(
                &self,
                user_id: ::uuid::Uuid,
                family: $crate::gateway::Family,
                model: Option<&str>,
                pool_id: ::uuid::Uuid,
            ) -> Result<(String, ::uuid::Uuid), $crate::account_resolve::ResolveError> {
                $crate::account_resolve::resolve_pool_by_id(self, user_id, family, model, pool_id)
                    .await
            }
            async fn stamp_pool(&self, session_id: &str, pool_id: ::uuid::Uuid) {
                $crate::account_resolve::stamp_pool(self, session_id, pool_id).await;
            }
        }
    };
}

#[macro_export]
macro_rules! impl_dispatch_mint {
    ($host:ty) => {
        impl $crate::hooks::DispatchMint for $host {
            async fn account_provider_rows(
                &self,
                user_id: ::uuid::Uuid,
                account_name: &str,
            ) -> Result<Option<Vec<$crate::gateway::mint::ProviderRow>>, ::sqlx::Error> {
                $crate::gateway::mint::account_provider_rows(self, user_id, account_name).await
            }
            async fn acting_user(
                &self,
                session_id: &str,
                known: Option<::uuid::Uuid>,
            ) -> Option<::uuid::Uuid> {
                $crate::gateway::mint::acting_user(self, session_id, known).await
            }
            async fn flip_model_redirect(
                &self,
                user_id: ::uuid::Uuid,
                provider_row_id: ::uuid::Uuid,
                family: $crate::gateway::Family,
                requested: &str,
            ) -> String {
                $crate::gateway::mint::flip_model_redirect(
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
                family: $crate::gateway::Family,
                session_id: &str,
            ) -> Result<
                ::std::collections::BTreeMap<String, String>,
                $crate::gateway::mint::MintSessionEnvError,
            > {
                $crate::gateway::mint::mint_session_env(
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
                primary: $crate::gateway::Family,
                session_id: &str,
            ) -> Result<
                ::std::collections::BTreeMap<String, String>,
                $crate::gateway::mint::MintSessionEnvError,
            > {
                $crate::gateway::mint::mint_session_env_all_families(
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
                $crate::gateway::mint::mint_session_env_for_account(self, provider_id, session_id)
                    .await
            }
            async fn resolve_account_model(
                &self,
                user_id: ::uuid::Uuid,
                account_name: &str,
                family: $crate::gateway::Family,
                model: &str,
            ) -> String {
                $crate::gateway::mint::resolve_account_model(
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
                $crate::gateway::mint::resolve_session_settings(self, session_id).await
            }
        }
    };
}

#[macro_export]
macro_rules! impl_dispatch_usage {
    ($host:ty) => {
        impl $crate::hooks::DispatchUsage for $host {
            async fn reload_account(
                &self,
                id: ::uuid::Uuid,
            ) -> Option<$crate::gateway::credentials::Account> {
                $crate::gateway::credentials::reload_account(self, id).await
            }
            async fn session_spend_usd(
                &self,
                account_id: ::uuid::Uuid,
                session_id: &str,
            ) -> Option<f64> {
                $crate::gateway::usage::session_spend_usd(self, account_id, session_id).await
            }
            async fn usage_for_soft_limit(
                &self,
                account_id: ::uuid::Uuid,
            ) -> Option<::serde_json::Value> {
                $crate::gateway::usage::usage_for_soft_limit(self, account_id).await
            }
        }
    };
}

#[macro_export]
macro_rules! impl_dispatch_account_store {
    ($host:ty, $pool:ident) => {
        impl $crate::hooks::DispatchAccountStore for $host {
            async fn account_rules(
                &self,
                session_id: &str,
            ) -> Result<Vec<::cctui_proto::hook_policy::HookRule>, ::sqlx::Error> {
                $crate::hook_policy::account_rules(&self.$pool, session_id).await
            }
            async fn for_child(
                &self,
                parent_session_id: &str,
                additions: Vec<::cctui_proto::hook_policy::HookRule>,
            ) -> Result<::cctui_proto::hook_policy::HookPolicy, $crate::hook_policy::PolicyError>
            {
                $crate::hook_policy::for_child(&self.$pool, parent_session_id, additions).await
            }
            async fn for_dispatch(
                &self,
                session_id: &str,
                adapter: &str,
                payload: &mut ::serde_json::Value,
            ) -> Result<(), $crate::hook_policy::PolicyError> {
                $crate::hook_policy::for_dispatch(&self.$pool, session_id, adapter, payload).await
            }
            async fn record_hook_policy(
                &self,
                command: ::uuid::Uuid,
                carried: &::cctui_proto::hook_policy::SessionHookPolicy,
            ) -> Result<(), ::sqlx::Error> {
                $crate::hook_policy::record(&self.$pool, command, carried).await
            }
            async fn account_pool_by_name(
                &self,
                user_id: ::uuid::Uuid,
                name: &str,
            ) -> Result<Option<::cctui_proto::api::account_pools::AccountPool>, ::sqlx::Error> {
                $crate::store::account_pools::by_name(&self.$pool, user_id, name).await
            }
            async fn live_for_launch(
                &self,
                user_id: ::uuid::Uuid,
            ) -> Result<Vec<::cctui_proto::api::account_redirects::AccountRedirect>, ::sqlx::Error>
            {
                $crate::store::account_redirects::live_for_launch(&self.$pool, user_id).await
            }
        }
    };
}
