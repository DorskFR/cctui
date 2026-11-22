//! Calls into other domains; the host forwards each method to the owning
//! crate's function of the same name.
use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;

use cctui_proto::api::account_pools::AccountPool;
use cctui_proto::api::account_redirects::AccountRedirect;
use cctui_proto::hook_policy::{HookPolicy, HookRule, SessionHookPolicy};
use cctui_server_kernel::accounts_types::{
    Account, Bound, Family, MintSessionEnvError, PolicyError, ProviderRow, ResolveError,
};
use cctui_server_kernel::bus::BusError;
use uuid::Uuid;

pub trait DispatchAccountResolution: Sync {
    fn in_flight_by_provider(
        &self,
        providers: &[Uuid],
    ) -> impl Future<Output = HashMap<Uuid, u32>> + Send;
    fn needing_reauth(&self, providers: &[Uuid]) -> impl Future<Output = HashSet<Uuid>> + Send;
    fn resolve_account_or_pool(
        &self,
        user_id: Uuid,
        family: Family,
        model: Option<&str>,
        name: &str,
    ) -> impl Future<Output = Result<Bound, ResolveError>> + Send;
    fn resolve_pool(
        &self,
        user_id: Uuid,
        family: Family,
        model: Option<&str>,
        pool_name: &str,
    ) -> impl Future<Output = Result<(String, Uuid), ResolveError>> + Send;
    fn resolve_pool_by_id(
        &self,
        user_id: Uuid,
        family: Family,
        model: Option<&str>,
        pool_id: Uuid,
    ) -> impl Future<Output = Result<(String, Uuid), ResolveError>> + Send;
    fn stamp_pool(&self, session_id: &str, pool_id: Uuid) -> impl Future<Output = ()> + Send;
}

pub trait DispatchMint: Sync {
    fn account_provider_rows(
        &self,
        user_id: Uuid,
        account_name: &str,
    ) -> impl Future<Output = Result<Option<Vec<ProviderRow>>, sqlx::Error>> + Send;
    fn acting_user(
        &self,
        session_id: &str,
        known: Option<Uuid>,
    ) -> impl Future<Output = Option<Uuid>> + Send;
    fn flip_model_redirect(
        &self,
        user_id: Uuid,
        provider_row_id: Uuid,
        family: Family,
        requested: &str,
    ) -> impl Future<Output = String> + Send;
    fn mint_session_env(
        &self,
        user_id: Uuid,
        account_name: &str,
        family: Family,
        session_id: &str,
    ) -> impl Future<Output = Result<BTreeMap<String, String>, MintSessionEnvError>> + Send;
    fn mint_session_env_all_families(
        &self,
        user_id: Uuid,
        account_name: &str,
        primary: Family,
        session_id: &str,
    ) -> impl Future<Output = Result<BTreeMap<String, String>, MintSessionEnvError>> + Send;
    fn mint_session_env_for_account(
        &self,
        provider_id: Uuid,
        session_id: &str,
    ) -> impl Future<Output = Result<Option<BTreeMap<String, String>>, sqlx::Error>> + Send;
    fn resolve_account_model(
        &self,
        user_id: Uuid,
        account_name: &str,
        family: Family,
        model: &str,
    ) -> impl Future<Output = String> + Send;
    fn resolve_session_settings(
        &self,
        session_id: &str,
    ) -> impl Future<Output = Option<serde_json::Value>> + Send;
}

pub trait DispatchUsage: Sync {
    fn reload_account(&self, id: Uuid) -> impl Future<Output = Option<Account>> + Send;
    fn session_spend_usd(
        &self,
        account_id: Uuid,
        session_id: &str,
    ) -> impl Future<Output = Option<f64>> + Send;
    fn usage_for_soft_limit(
        &self,
        account_id: Uuid,
    ) -> impl Future<Output = Option<serde_json::Value>> + Send;
}

pub trait DispatchAccountStore: Sync {
    fn account_rules(
        &self,
        session_id: &str,
    ) -> impl Future<Output = Result<Vec<HookRule>, sqlx::Error>> + Send;
    fn for_child(
        &self,
        parent_session_id: &str,
        additions: Vec<HookRule>,
    ) -> impl Future<Output = Result<HookPolicy, PolicyError>> + Send;
    fn for_dispatch(
        &self,
        session_id: &str,
        adapter: &str,
        payload: &mut serde_json::Value,
    ) -> impl Future<Output = Result<(), PolicyError>> + Send;
    /// `hook_policy::record`.
    fn record_hook_policy(
        &self,
        command: Uuid,
        carried: &SessionHookPolicy,
    ) -> impl Future<Output = Result<(), sqlx::Error>> + Send;
    /// `store::account_pools::by_name`.
    fn account_pool_by_name(
        &self,
        user_id: Uuid,
        name: &str,
    ) -> impl Future<Output = Result<Option<AccountPool>, sqlx::Error>> + Send;
    fn live_for_launch(
        &self,
        user_id: Uuid,
    ) -> impl Future<Output = Result<Vec<AccountRedirect>, sqlx::Error>> + Send;
}

pub trait DispatchSessions: Sync {
    fn push_reconcile(&self, machine_id: Uuid)
    -> impl Future<Output = Result<(), BusError>> + Send;
    fn enabled_adapter_ids(
        &self,
        machine_id: Uuid,
    ) -> impl Future<Output = sqlx::Result<Vec<String>>> + Send;
}

pub trait DispatchPresence: Sync {
    /// This replica's `ws_presence.pod`.
    fn presence_pod(&self) -> &str;
}

pub trait DispatchNotify: Sync {
    /// Resolve the open notices of dismissed or re-run failed dispatches.
    fn resolve_notices(
        &self,
        user_id: Uuid,
        session_ids: &[String],
    ) -> impl Future<Output = ()> + Send;
}

pub trait DispatchSessionArchive: Sync {
    fn archive_one(
        &self,
        session_id: &str,
        force: bool,
        initiator: cctui_proto::adapter::RemoveInitiator,
        actor: cctui_server_kernel::events::Actor,
    ) -> impl Future<
        Output = Result<cctui_server_kernel::sessions_types::ArchiveOutcome, sqlx::Error>,
    > + Send;
    fn any_viewed(
        &self,
        sessions: &[String],
    ) -> impl Future<Output = Result<bool, sqlx::Error>> + Send;
    /// Whether a session in `tree` waits on a permission prompt or plan.
    fn prompting(&self, tree: &[String]) -> impl Future<Output = bool> + Send;
}
