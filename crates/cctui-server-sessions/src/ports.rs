//! Calls the sessions code makes into other domains.
//!
//! The host implements each
//! trait by forwarding to the owner's function named in the method's doc line;
//! that function's leading state or pool argument is `&self`.
use std::collections::BTreeMap;
use std::future::Future;

use axum::Json;
use axum::http::StatusCode;
use cctui_proto::adapter::{AdapterEvent, RateLimitWindow, SessionSpec};
use cctui_proto::api::capabilities::LangfuseCapability;
use cctui_proto::api::outcome::{DeclareOutcomeRequest, SessionOutcome};
use cctui_proto::api::uploads::UploadCaps;
use cctui_proto::api::{
    AddUserActionRequest, ApiError, ArchiveChildResponse, DispatchRequest, MessageChildRequest,
    SessionPlugin, SpawnCapability, SpawnChildRequest, SpawnChildResponse, SpawnRequest,
    UserAction, UserActionResolver, UserActionResult, UserActionStatus,
};
use cctui_proto::harness::HarnessReport;
use cctui_proto::hook_policy::SessionHookPolicy;
use cctui_proto::ids::{SessionId, SpawnKey};
use cctui_proto::models::MachineLiveness;
use cctui_proto::resources::MachineResources;
use cctui_proto::ws::DaemonFrameUp;
use cctui_server_kernel::accounts_types::{Account, PolicyError, SessionBinding};
use cctui_server_kernel::auth::AuthContext;
use cctui_server_kernel::blobs_types::StoredBlob;
use cctui_server_kernel::bus::BusError;
use cctui_server_kernel::dispatch_types::{
    AccountTarget, BoundAccount, DispatchResult, Forwarded, Launch, LaunchError, LimitsQuery,
    MachineSpawn, Parent, SessionLimits,
};
use cctui_server_kernel::error::AppError;
use cctui_server_kernel::extras_types::Kind;
use cctui_server_kernel::notify_types::{NewNotification, Ticked};
use cctui_server_kernel::search_types::Chunked;
use serde_json::Value;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

/// Accounts: credentials, gateway env, hook policy and usage.
pub trait SessionsAccounts: Send + Sync + 'static {
    /// `gateway::mint::resume_env_for_session`.
    fn resume_env_for_session(
        &self,
        session_id: &str,
    ) -> impl Future<Output = BTreeMap<String, String>> + Send;
    /// `gateway::mint::resolve_session_accounts`.
    fn resolve_session_accounts(&self, session_id: &str) -> impl Future<Output = Vec<Uuid>> + Send;
    /// `gateway::usage::usage_for_soft_limit`.
    fn usage_for_soft_limit(&self, account_id: Uuid) -> impl Future<Output = Option<Value>> + Send;
    /// `gateway::credentials::current_access_token`.
    fn current_access_token(
        &self,
        acct: &Account,
    ) -> impl Future<Output = Result<String, StatusCode>> + Send;
    /// `gateway::credentials::reload_account`.
    fn reload_account(&self, id: Uuid) -> impl Future<Output = Option<Account>> + Send;
    /// `gateway::mint::acting_user`.
    fn acting_user(
        &self,
        session_id: &str,
        known: Option<Uuid>,
    ) -> impl Future<Output = Option<Uuid>> + Send;
    /// `gateway::limits::clear_orphan_block_for_session`.
    fn clear_orphan_block_for_session(&self, session_id: &str) -> impl Future<Output = ()> + Send;
    /// `gateway::limits::clear_soft_limit_block`.
    fn clear_soft_limit_block(&self, session_id: &str) -> impl Future<Output = ()> + Send;
    /// `routes::gateway::fork::fork_env_for_session`.
    fn fork_env_for_session(
        &self,
        parent: &str,
        child: &str,
    ) -> impl Future<Output = Result<BTreeMap<String, String>, AppError>> + Send;
    /// `gateway::mint::resolve_session_settings`.
    fn resolve_session_settings(
        &self,
        session_id: &str,
    ) -> impl Future<Output = Option<Value>> + Send;
    /// `session_credentials::revoke_session_tokens`.
    fn revoke_session_tokens(&self, session_id: &str) -> impl Future<Output = ()> + Send;
    /// `gateway::mint::revoke_grantee_session_tokens`.
    fn revoke_grantee_session_tokens(
        &self,
        account_id: Uuid,
        grantee: Uuid,
    ) -> impl Future<Output = ()> + Send;
    /// `gateway::mint::resolve_session_binding`.
    fn resolve_session_binding(
        &self,
        session_id: &str,
    ) -> impl Future<Output = SessionBinding> + Send;
    /// `gateway::mint::mint_binding_env`.
    fn mint_binding_env(
        &self,
        session_id: &str,
        binding: &SessionBinding,
    ) -> impl Future<Output = Result<BTreeMap<String, String>, sqlx::Error>> + Send;
    /// `gateway::mint::binding_settings`.
    fn binding_settings(
        &self,
        session_id: &str,
        binding: &SessionBinding,
    ) -> impl Future<Output = Option<Value>> + Send;
    /// `session_credentials::rebind_spawn_key`.
    fn rebind_spawn_key(
        &self,
        spawn_key: SpawnKey,
        session_id: SessionId,
    ) -> impl Future<Output = ()> + Send;
    /// `usage_history::record_agent_limits`.
    fn record_agent_limits(
        &self,
        session_id: String,
        windows: &[RateLimitWindow],
        observed_at: Option<i64>,
    );
    /// `daemon_usage::on_token_usage`.
    fn on_token_usage(&self, event: AdapterEvent) -> impl Future<Output = sqlx::Result<()>> + Send;
    /// `hook_policy::inherit_for_launch`.
    fn inherit_for_launch(
        &self,
        parent_session_id: &str,
        child_key: &str,
        spec: &mut SessionSpec,
    ) -> impl Future<Output = Result<SessionHookPolicy, PolicyError>> + Send;
    /// `hook_policy::for_session`.
    fn hook_policy_for_session(
        &self,
        session_id: &str,
    ) -> impl Future<Output = Result<SessionHookPolicy, sqlx::Error>> + Send;
}

/// Blobs: image externalization and attachment storage.
pub trait SessionsBlobs: Send + Sync + 'static {
    /// `routes::store_blob_in`.
    fn store_blob_in(
        conn: &mut PgConnection,
        bytes: &[u8],
        media_type: Option<&str>,
    ) -> impl Future<Output = Result<StoredBlob, sqlx::Error>> + Send;
    /// `routes::fetch_session_blob`.
    #[allow(clippy::type_complexity)]
    fn fetch_session_blob(
        pool: &PgPool,
        session_id: &str,
        hash: &str,
    ) -> impl Future<Output = Result<Option<(Option<String>, Vec<u8>)>, sqlx::Error>> + Send;
    /// `image_blobs::externalize`.
    fn externalize(
        &self,
        session_id: &str,
        event_type: &str,
        payload: &mut Value,
    ) -> impl Future<Output = bool> + Send;
    /// `image_blobs::externalize_event`.
    fn externalize_event(&self, event: &mut AdapterEvent) -> impl Future<Output = ()> + Send;
    /// `image_blobs::inline_client_rows`.
    fn inline_client_rows(
        &self,
        session_id: &str,
        rows: &mut [Value],
        budget: usize,
    ) -> impl Future<Output = Result<(), sqlx::Error>> + Send;
}

/// Dispatch: launches, children, machine policy and liveness.
pub trait SessionsDispatch: Send + Sync + 'static {
    /// `child_exec::capability_for`.
    fn capability_for(
        &self,
        session_id: &str,
    ) -> impl Future<Output = Option<SpawnCapability>> + Send;
    /// `child_exec::child_spawner`.
    fn child_spawner(&self, parent: &Parent) -> impl Future<Output = Uuid> + Send;
    /// `child_exec::load_parent`.
    fn load_parent(
        &self,
        session_id: &str,
        caller: Uuid,
    ) -> impl Future<Output = Result<Parent, AppError>> + Send;
    /// `child_exec::spawn_child`.
    fn spawn_child(
        &self,
        caller: Uuid,
        session_id: &str,
        req: &SpawnChildRequest,
    ) -> impl Future<Output = Result<SpawnChildResponse, AppError>> + Send;
    /// `child_exec::message_child`.
    fn message_child(
        &self,
        caller: Uuid,
        session_id: &str,
        req: &MessageChildRequest,
    ) -> impl Future<Output = Result<(), AppError>> + Send;
    /// `child_exec::deliver_completion`.
    fn deliver_completion(
        &self,
        caller: Uuid,
        child_id: &str,
        text: &str,
    ) -> impl Future<Output = Result<String, AppError>> + Send;
    /// `child_exec::archive_descendant`.
    fn archive_descendant(
        &self,
        caller: Uuid,
        session_id: &str,
        target: &str,
    ) -> impl Future<Output = Result<ArchiveChildResponse, AppError>> + Send;
    /// `child_policy::pending_child_count`.
    fn pending_child_count(
        &self,
        parent_id: &str,
    ) -> impl Future<Output = Result<u32, sqlx::Error>> + Send;
    /// `session_limits::inspect`.
    fn session_limits_inspect(
        &self,
        caller: Uuid,
        session_id: &str,
        q: &LimitsQuery,
    ) -> impl Future<Output = Result<SessionLimits, AppError>> + Send;
    /// `launch::launch_durable`.
    fn launch_durable(
        &self,
        actor: Uuid,
        spawn: MachineSpawn,
    ) -> impl Future<Output = Result<(), LaunchError>> + Send;
    /// `launch::no_daemon_detail`.
    fn no_daemon_detail(&self, machine: Uuid) -> impl Future<Output = &'static str> + Send;
    /// `launch::delivery_detail`.
    fn delivery_detail(
        &self,
        machine: Uuid,
        error: &BusError,
    ) -> impl Future<Output = &'static str> + Send;
    /// `machine_spawn::resolve_spawn_account`.
    fn resolve_spawn_account(
        &self,
        target: &AccountTarget,
        req: &SpawnRequest,
    ) -> impl Future<Output = Result<BoundAccount, (StatusCode, Json<ApiError>)>> + Send;
    /// `machine_policy::resolve_spawn_machine`.
    fn resolve_spawn_machine(
        &self,
        ctx: &AuthContext,
        machine_id: &str,
    ) -> impl Future<Output = Result<(Uuid, Uuid), (StatusCode, Json<ApiError>)>> + Send;
    /// `machine_policy::spawn_adapter_on`.
    fn spawn_adapter_on(
        &self,
        machine_uuid: Uuid,
        requested: Option<&str>,
    ) -> impl Future<Output = Result<String, (StatusCode, Json<ApiError>)>> + Send;
    /// `machine_policy::persist_spawn_capability`.
    fn persist_spawn_capability(
        &self,
        machine_uuid: Uuid,
        req: &SpawnRequest,
        permission_mode: Option<cctui_proto::adapter::PermissionMode>,
        token_session_id: &str,
    ) -> impl Future<Output = ()> + Send;
    /// `machine_policy::authorize_launch`.
    fn authorize_launch(
        &self,
        ctx: &AuthContext,
        machine_uuid: Uuid,
    ) -> impl Future<Output = Result<(), AppError>> + Send;
    /// `machine_policy::resolve_owned_machine`.
    fn resolve_owned_machine(
        &self,
        ctx: &AuthContext,
        machine_id: &str,
    ) -> impl Future<Output = Result<(Uuid, Uuid), (StatusCode, Json<ApiError>)>> + Send;
    /// `machine_policy::claim_spawner`.
    fn claim_spawner(
        &self,
        session_id: &str,
        spawn_key: Option<&str>,
    ) -> impl Future<Output = ()> + Send;
    /// `machine_policy::fill_spawner`.
    fn fill_spawner(&self, session_id: &str, user_id: Uuid) -> impl Future<Output = ()> + Send;
    /// `machine_policy::remember_spawner`.
    fn remember_spawner(
        &self,
        spawn_key: &str,
        user_id: Uuid,
        name: Option<&str>,
    ) -> impl Future<Output = ()> + Send;
    /// `machine_policy::validate_env`.
    fn validate_env(&self, req: &SpawnRequest) -> Result<(), (StatusCode, Json<ApiError>)>;
    /// `launch_credentials::gateway_env_allowed`.
    fn gateway_env_allowed(
        &self,
        user_id: Uuid,
        machine_id: Uuid,
        session_id: &str,
    ) -> impl Future<Output = Result<bool, sqlx::Error>> + Send;
    /// `launch_credentials::spawn_capability_for`.
    fn spawn_capability_for(
        &self,
        session_id: &str,
    ) -> impl Future<Output = Option<SpawnCapability>> + Send;
    /// `launch_credentials::token_valid`.
    fn token_valid(
        &self,
        user_id: Uuid,
        session_id: &str,
        hash: &str,
    ) -> impl Future<Output = Result<Option<bool>, sqlx::Error>> + Send;
    /// `webhook::queue_outcome`.
    fn queue_outcome(
        &self,
        session_id: &str,
        outcome: &DeclareOutcomeRequest,
    ) -> impl Future<Output = sqlx::Result<bool>> + Send;
    /// `machine_liveness::record_and_broadcast`.
    fn machine_liveness_record_and_broadcast(
        &self,
        machine_id: Uuid,
        tier: MachineLiveness,
    ) -> bool;
    /// `machine_resources::record_and_broadcast`.
    fn machine_resources_record_and_broadcast(
        &self,
        machine_id: Uuid,
        resources: MachineResources,
    ) -> impl Future<Output = ()> + Send;
    /// `routes::harness_update::on_heartbeat`.
    fn on_heartbeat(
        &self,
        machine_id: Uuid,
        report: &HarnessReport,
    ) -> impl Future<Output = ()> + Send;
    /// `routes::dispatch::cancel_dispatched_workload`.
    fn cancel_dispatched_workload(&self, session_id: &str) -> impl Future<Output = bool> + Send;
    /// `machine_launches::find`.
    fn machine_launches_find(
        &self,
        identity: &str,
        actor: Option<Uuid>,
    ) -> impl Future<Output = Result<Option<Launch>, sqlx::Error>> + Send;
    /// `machine_launches::verdict`.
    fn machine_launches_verdict(
        &self,
        command: Uuid,
        machine: Uuid,
        owner: Uuid,
        ok: bool,
        detail: Option<&str>,
    ) -> impl Future<Output = Result<Option<Launch>, sqlx::Error>> + Send;
    /// `machine_launches::registered`.
    fn machine_launches_registered(
        &self,
        key: &str,
        session: &str,
        machine: Uuid,
        owner: Uuid,
    ) -> impl Future<Output = Result<(), sqlx::Error>> + Send;
    /// `dispatch::execute_dispatch`.
    fn execute_dispatch(
        &self,
        ctx: &AuthContext,
        req: DispatchRequest,
    ) -> impl Future<Output = DispatchResult<Forwarded>> + Send;
    /// `dispatch_archive::clamp`.
    fn dispatch_archive_clamp(&self, data: &mut Value);
}

/// Extras: presence, previews, plugins, the cctuiverse and Langfuse.
pub trait SessionsExtras: Send + Sync + 'static {
    /// `presence::register_conn`.
    fn register_conn(
        &self,
        kind: Kind,
        entity_id: Uuid,
        conn: Uuid,
    ) -> impl Future<Output = ()> + Send;
    /// `presence::unregister_conn`.
    fn unregister_conn(
        &self,
        kind: Kind,
        entity_id: Uuid,
        conn: Uuid,
    ) -> impl Future<Output = ()> + Send;
    /// This pod's presence id.
    fn presence_pod(&self) -> &str;
    /// `preview::Registry::detach_session`.
    fn preview_detach_session(&self, session_id: &str) -> impl Future<Output = ()> + Send;
    /// `preview::Registry::detach_machine`.
    fn preview_detach_machine(&self, machine_id: Uuid) -> impl Future<Output = ()> + Send;
    /// `preview::on_frame`.
    fn preview_on_frame(
        &self,
        machine_id: Uuid,
        daemon_user: Uuid,
        frame: DaemonFrameUp,
    ) -> impl Future<Output = ()> + Send;
    /// `preview::close_session`.
    fn preview_close_session(&self, session_id: &str) -> impl Future<Output = ()> + Send;
    /// `cctuiverse::turn_ended`.
    fn turn_ended(&self, session_id: &str);
    /// What `GET /capabilities` reports about the Langfuse client.
    fn langfuse_capability(&self) -> impl Future<Output = LangfuseCapability> + Send;
    /// The setting keys an installed plugin declares; `None` when it is not installed.
    fn plugin_setting_keys(&self, id: &str) -> Option<Vec<String>>;
    /// `plugin_host_token::reconcile`.
    fn plugin_host_token_reconcile(
        &self,
        user_id: Uuid,
        prev: Option<&Value>,
        next: &mut Value,
    ) -> impl Future<Output = sqlx::Result<()>> + Send;
    /// `plugin_host_token::strip`.
    fn plugin_host_token_strip(&self, data: &mut Value);
    /// `plugins::session_plugins`.
    fn session_plugins(&self, settings: Option<&Value>) -> Vec<SessionPlugin>;
    /// `plugin_store::sync_or_warn`.
    fn sync_or_warn(&self) -> impl Future<Output = ()> + Send;
}

/// Host-owned instance settings.
pub trait SessionsHost: Send + Sync + 'static {
    /// `routes::server_settings::cached_upload_caps`.
    fn cached_upload_caps(&self) -> UploadCaps;
    /// `routes::update_hook::record_hook_flag`.
    fn record_hook_flag(&self, machine: Uuid, has_hook: bool) -> impl Future<Output = ()> + Send;
}

/// Notify: notifications and the user-action list.
pub trait SessionsNotify: Send + Sync + 'static {
    /// `notify::emit_detached`.
    fn emit_detached<F>(&self, owner: Option<Uuid>, build: F)
    where
        F: FnOnce(Uuid) -> NewNotification + Send + 'static;
    /// `notify::emit_now`, on its own task.
    fn alert(&self, notice: NewNotification);
    /// `notify::producers::on_outcome`.
    fn on_outcome(&self, outcome: &SessionOutcome) -> impl Future<Output = ()> + Send;
    /// `notify::prefs::migrate_sound`.
    fn migrate_sound(&self, data: &mut Value);
    /// `notify::prefs::clamp_settings`.
    fn clamp_settings(
        &self,
        conn: &mut PgConnection,
        data: &mut Value,
    ) -> impl Future<Output = Result<(), String>> + Send;
    /// `routes::user_actions::list`.
    fn user_actions_list(
        &self,
        session_id: &str,
    ) -> impl Future<Output = Result<Vec<UserAction>, AppError>> + Send;
    /// `routes::user_actions::add`.
    fn user_actions_add(
        &self,
        session_id: &str,
        req: &AddUserActionRequest,
    ) -> impl Future<Output = Result<UserActionResult, AppError>> + Send;
    /// `routes::user_actions::tick`.
    fn user_actions_tick(
        &self,
        session_id: &str,
        id: Uuid,
        status: UserActionStatus,
        note: Option<&str>,
        by: UserActionResolver,
    ) -> impl Future<Output = Result<Ticked, AppError>> + Send;
}

/// Search: transcript indexing.
pub trait SessionsSearch: Send + Sync + 'static {
    /// `chunk_event`.
    fn chunk_event(&self, adapter_id: &str, event_type: &str, payload: Value) -> Vec<String>;
    /// `index_new`.
    fn index_new(&self, rows: Vec<Chunked>);
    /// `reindex`.
    fn reindex(&self, ids: &[i64]) -> impl Future<Output = Result<(), sqlx::Error>> + Send;
}
