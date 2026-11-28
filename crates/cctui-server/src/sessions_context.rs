//! The host's implementation of the sessions context and its ports.
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

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
use cctui_server_dispatch::child_exec::ChildExecutionContext;
use cctui_server_dispatch::child_policy::ChildCount;
use cctui_server_kernel::accounts_types::{Account, PolicyError, SessionBinding};
use cctui_server_kernel::auth::AuthContext;
use cctui_server_kernel::blobs_types::StoredBlob;
use cctui_server_kernel::bus::{Bus, BusError};
use cctui_server_kernel::dispatch_types::{
    AccountTarget, BoundAccount, ChildSlots, DispatchResult, Forwarded, Launch, LaunchError,
    LimitsQuery, MachineSpawn, Parent, SessionLimits,
};
use cctui_server_kernel::error::AppError;
use cctui_server_kernel::extras_types::Kind;
use cctui_server_kernel::notify_types::{NewNotification, Ticked};
use cctui_server_kernel::search_types::Chunked;
use cctui_server_sessions::bandwidth_watch::{ConnectTracker, DivergenceTracker, EvictionTracker};
use cctui_server_sessions::context::{DaemonContext, SessionsContext};
use cctui_server_sessions::ports::{
    SessionsAccounts, SessionsBlobs, SessionsDispatch, SessionsExtras, SessionsHost,
    SessionsNotify, SessionsSearch,
};
use cctui_server_sessions::routes::codex_models::{CachedCatalog, CachedVersion};
use cctui_server_sessions::routes::permissions::SharedPermissionStore;
use dashmap::DashMap;
use serde_json::Value;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::state::AppState;

impl DaemonContext for AppState {
    fn daemon_pool(&self) -> &PgPool {
        &self.core.pool
    }
    fn daemon_bus(&self) -> &Bus {
        &self.core.bus
    }
    fn mark_ack_daemons(&self) -> &Arc<DashMap<Uuid, ()>> {
        &self.core.mark_ack_daemons
    }
}

impl SessionsContext for AppState {
    fn permission_store(&self) -> &SharedPermissionStore {
        &self.permission_store
    }
    fn machine_event_inserts(&self) -> &Arc<DashMap<Uuid, u64>> {
        &self.core.machine_event_inserts
    }
    fn auth_recovery_daemons(&self) -> &Arc<DashMap<Uuid, ()>> {
        &self.core.auth_recovery_daemons
    }
    fn pr_status_cache(&self) -> &cctui_proto::classifier::PrStatusCache {
        &self.pr_status_cache
    }
    fn codex_catalogs(&self) -> &Arc<DashMap<Uuid, CachedCatalog>> {
        &self.codex_catalogs
    }
    fn codex_account_catalogs(&self) -> &Arc<DashMap<Uuid, CachedCatalog>> {
        &self.codex_account_catalogs
    }
    fn codex_latest_version(&self) -> &Arc<Mutex<Option<CachedVersion>>> {
        &self.codex_latest_version
    }
    fn eviction_tracker(&self) -> &Arc<EvictionTracker> {
        &self.eviction_tracker
    }
    fn connect_tracker(&self) -> &Arc<ConnectTracker> {
        &self.connect_tracker
    }
    fn divergence_tracker(&self) -> &Arc<DivergenceTracker> {
        &self.divergence_tracker
    }
}

impl SessionsAccounts for AppState {
    async fn resume_env_for_session(&self, session_id: &str) -> BTreeMap<String, String> {
        cctui_server_accounts::routes::gateway::resume_env_for_session(self, session_id).await
    }
    async fn resolve_session_accounts(&self, session_id: &str) -> Vec<Uuid> {
        cctui_server_accounts::routes::gateway::resolve_session_accounts(self, session_id).await
    }
    async fn usage_for_soft_limit(&self, account_id: Uuid) -> Option<Value> {
        cctui_server_accounts::routes::gateway::usage_for_soft_limit(self, account_id).await
    }
    async fn current_access_token(&self, acct: &Account) -> Result<String, StatusCode> {
        cctui_server_accounts::routes::gateway::current_access_token(self, acct).await
    }
    async fn reload_account(&self, id: Uuid) -> Option<Account> {
        cctui_server_accounts::routes::gateway::reload_account(self, id).await
    }
    async fn acting_user(&self, session_id: &str, known: Option<Uuid>) -> Option<Uuid> {
        cctui_server_accounts::routes::gateway::acting_user(self, session_id, known).await
    }
    async fn clear_orphan_block_for_session(&self, session_id: &str) {
        cctui_server_accounts::routes::gateway::clear_orphan_block_for_session(self, session_id)
            .await;
    }
    async fn clear_soft_limit_block(&self, session_id: &str) {
        cctui_server_accounts::routes::gateway::clear_soft_limit_block(self, session_id).await;
    }
    async fn fork_env_for_session(
        &self,
        parent: &str,
        child: &str,
    ) -> Result<BTreeMap<String, String>, AppError> {
        cctui_server_accounts::routes::gateway::fork_env_for_session(self, parent, child).await
    }
    async fn resolve_session_settings(&self, session_id: &str) -> Option<Value> {
        cctui_server_accounts::routes::gateway::resolve_session_settings(self, session_id).await
    }
    async fn revoke_session_tokens(&self, session_id: &str) {
        cctui_server_accounts::routes::gateway::revoke_session_tokens(self, session_id).await;
    }
    async fn revoke_grantee_session_tokens(&self, account_id: Uuid, grantee: Uuid) {
        cctui_server_accounts::routes::accounts::revoke_grantee_session_tokens(
            &self.pool, account_id, grantee,
        )
        .await;
    }
    async fn resolve_session_binding(&self, session_id: &str) -> SessionBinding {
        cctui_server_accounts::routes::gateway::resolve_session_binding(self, session_id).await
    }
    async fn mint_binding_env(
        &self,
        session_id: &str,
        binding: &SessionBinding,
    ) -> Result<BTreeMap<String, String>, sqlx::Error> {
        cctui_server_accounts::routes::gateway::mint_binding_env(self, session_id, binding).await
    }
    async fn binding_settings(&self, session_id: &str, binding: &SessionBinding) -> Option<Value> {
        cctui_server_accounts::routes::gateway::binding_settings(self, session_id, binding).await
    }
    async fn rebind_spawn_key(&self, spawn_key: SpawnKey, session_id: SessionId) {
        cctui_server_accounts::routes::gateway::rebind_spawn_key(self, spawn_key, session_id).await;
    }
    fn record_agent_limits(
        &self,
        session_id: String,
        windows: &[RateLimitWindow],
        observed_at: Option<i64>,
    ) {
        cctui_server_accounts::usage_history::record_agent_limits(
            self,
            session_id,
            windows,
            observed_at,
        );
    }
    async fn on_token_usage(&self, event: AdapterEvent) -> sqlx::Result<()> {
        cctui_server_accounts::daemon_usage::on_token_usage(&self.pool, event).await
    }
    async fn inherit_for_launch(
        &self,
        parent_session_id: &str,
        child_key: &str,
        spec: &mut SessionSpec,
    ) -> Result<SessionHookPolicy, PolicyError> {
        cctui_server_accounts::hook_policy::inherit_for_launch(
            &self.pool,
            parent_session_id,
            child_key,
            spec,
        )
        .await
    }
    async fn hook_policy_for_session(
        &self,
        session_id: &str,
    ) -> Result<SessionHookPolicy, sqlx::Error> {
        cctui_server_accounts::hook_policy::for_session(&self.pool, session_id).await
    }
}

impl SessionsBlobs for AppState {
    async fn store_blob_in(
        conn: &mut PgConnection,
        bytes: &[u8],
        media_type: Option<&str>,
    ) -> Result<StoredBlob, sqlx::Error> {
        cctui_server_blobs::routes::store_blob_in(conn, bytes, media_type).await
    }
    async fn fetch_session_blob(
        pool: &PgPool,
        session_id: &str,
        hash: &str,
    ) -> Result<Option<(Option<String>, Vec<u8>)>, sqlx::Error> {
        cctui_server_blobs::routes::fetch_session_blob(pool, session_id, hash).await
    }
    async fn externalize(&self, session_id: &str, event_type: &str, payload: &mut Value) -> bool {
        cctui_server_blobs::image_blobs::externalize(&self.pool, session_id, event_type, payload)
            .await
    }
    async fn externalize_event(&self, event: &mut AdapterEvent) {
        cctui_server_blobs::image_blobs::externalize_event(&self.pool, event).await;
    }
    async fn inline_client_rows(
        &self,
        session_id: &str,
        rows: &mut [Value],
        budget: usize,
    ) -> Result<(), sqlx::Error> {
        cctui_server_blobs::image_blobs::inline_client_rows(&self.pool, session_id, rows, budget)
            .await
    }
}

impl SessionsDispatch for AppState {
    async fn capability_for(&self, session_id: &str) -> Option<SpawnCapability> {
        cctui_server_dispatch::child_exec::capability_for(self, session_id).await
    }
    async fn child_spawner(&self, parent: &Parent) -> Uuid {
        cctui_server_dispatch::child_exec::child_spawner(&self.pool, parent).await
    }
    async fn load_parent(&self, session_id: &str, caller: Uuid) -> Result<Parent, AppError> {
        cctui_server_dispatch::child_exec::load_parent(self, session_id, caller).await
    }
    async fn spawn_child(
        &self,
        caller: Uuid,
        session_id: &str,
        req: &SpawnChildRequest,
    ) -> Result<SpawnChildResponse, AppError> {
        cctui_server_dispatch::child_exec::spawn_child(self, caller, session_id, req).await
    }
    async fn message_child(
        &self,
        caller: Uuid,
        session_id: &str,
        req: &MessageChildRequest,
    ) -> Result<(), AppError> {
        cctui_server_dispatch::child_exec::message_child(self, caller, session_id, req).await
    }
    async fn deliver_completion(
        &self,
        caller: Uuid,
        child_id: &str,
        text: &str,
    ) -> Result<String, AppError> {
        cctui_server_dispatch::child_exec::deliver_completion(self, caller, child_id, text).await
    }
    async fn archive_descendant(
        &self,
        caller: Uuid,
        session_id: &str,
        target: &str,
    ) -> Result<ArchiveChildResponse, AppError> {
        cctui_server_dispatch::child_exec::archive_descendant(self, caller, session_id, target)
            .await
    }
    async fn pending_child_count(&self, parent_id: &str) -> Result<u32, sqlx::Error> {
        cctui_server_dispatch::child_policy::pending_child_count(&self.pool, parent_id).await
    }
    async fn session_limits_inspect(
        &self,
        caller: Uuid,
        session_id: &str,
        q: &LimitsQuery,
    ) -> Result<SessionLimits, AppError> {
        cctui_server_dispatch::session_limits::inspect(self, caller, session_id, q).await
    }
    async fn launch_durable(&self, actor: Uuid, spawn: MachineSpawn) -> Result<(), LaunchError> {
        cctui_server_dispatch::launch::launch_durable(&self.core, actor, spawn).await
    }
    async fn no_daemon_detail(&self, machine: Uuid) -> &'static str {
        cctui_server_dispatch::launch::no_daemon_detail(&self.pool, machine).await
    }
    async fn delivery_detail(&self, machine: Uuid, error: &BusError) -> &'static str {
        cctui_server_dispatch::launch::delivery_detail(&self.pool, machine, error).await
    }
    async fn resolve_spawn_account(
        &self,
        target: &AccountTarget,
        req: &SpawnRequest,
    ) -> Result<BoundAccount, (StatusCode, Json<ApiError>)> {
        cctui_server_dispatch::machine_spawn::resolve_spawn_account(self, target, req).await
    }
    async fn resolve_spawn_machine(
        &self,
        ctx: &AuthContext,
        machine_id: &str,
    ) -> Result<(Uuid, Uuid), (StatusCode, Json<ApiError>)> {
        cctui_server_dispatch::machine_policy::resolve_spawn_machine(self, ctx, machine_id).await
    }
    async fn spawn_adapter_on(
        &self,
        machine_uuid: Uuid,
        requested: Option<&str>,
    ) -> Result<String, (StatusCode, Json<ApiError>)> {
        cctui_server_dispatch::machine_policy::spawn_adapter_on(self, machine_uuid, requested).await
    }
    async fn persist_spawn_capability(
        &self,
        machine_uuid: Uuid,
        req: &SpawnRequest,
        permission_mode: Option<cctui_proto::adapter::PermissionMode>,
        token_session_id: &str,
    ) {
        cctui_server_dispatch::machine_policy::persist_spawn_capability(
            self,
            machine_uuid,
            req,
            permission_mode,
            token_session_id,
        )
        .await;
    }
    async fn authorize_launch(
        &self,
        ctx: &AuthContext,
        machine_uuid: Uuid,
    ) -> Result<(), AppError> {
        cctui_server_dispatch::machine_policy::authorize_launch(self, ctx, machine_uuid).await
    }
    async fn resolve_owned_machine(
        &self,
        ctx: &AuthContext,
        machine_id: &str,
    ) -> Result<(Uuid, Uuid), (StatusCode, Json<ApiError>)> {
        cctui_server_dispatch::machine_policy::resolve_owned_machine(self, ctx, machine_id).await
    }
    async fn claim_spawner(&self, session_id: &str, spawn_key: Option<&str>) {
        cctui_server_dispatch::machine_policy::claim_spawner(&self.pool, session_id, spawn_key)
            .await;
    }
    async fn fill_spawner(&self, session_id: &str, user_id: Uuid) {
        cctui_server_dispatch::machine_policy::fill_spawner(&self.pool, session_id, user_id).await;
    }
    async fn remember_spawner(&self, spawn_key: &str, user_id: Uuid, name: Option<&str>) {
        cctui_server_dispatch::machine_policy::remember_spawner(
            &self.pool, spawn_key, user_id, name,
        )
        .await;
    }
    fn validate_env(&self, req: &SpawnRequest) -> Result<(), (StatusCode, Json<ApiError>)> {
        cctui_server_dispatch::machine_policy::validate_env(req)
    }
    async fn gateway_env_allowed(
        &self,
        user_id: Uuid,
        machine_id: Uuid,
        session_id: &str,
    ) -> Result<bool, sqlx::Error> {
        cctui_server_dispatch::launch_credentials::gateway_env_allowed(
            &self.pool, user_id, machine_id, session_id,
        )
        .await
    }
    async fn spawn_capability_for(&self, session_id: &str) -> Option<SpawnCapability> {
        cctui_server_dispatch::launch_credentials::spawn_capability_for(self, session_id).await
    }
    async fn token_valid(
        &self,
        user_id: Uuid,
        session_id: &str,
        hash: &str,
    ) -> Result<Option<bool>, sqlx::Error> {
        cctui_server_dispatch::launch_credentials::token_valid(
            &self.pool, user_id, session_id, hash,
        )
        .await
    }
    async fn queue_outcome(
        &self,
        session_id: &str,
        outcome: &DeclareOutcomeRequest,
    ) -> sqlx::Result<bool> {
        cctui_server_dispatch::webhook::queue_outcome(&self.pool, session_id, outcome).await
    }
    fn machine_liveness_record_and_broadcast(
        &self,
        machine_id: Uuid,
        tier: MachineLiveness,
    ) -> bool {
        cctui_server_dispatch::machine_liveness::record_and_broadcast(self, machine_id, tier)
    }
    async fn machine_resources_record_and_broadcast(
        &self,
        machine_id: Uuid,
        resources: MachineResources,
    ) {
        cctui_server_dispatch::machine_resources::record_and_broadcast(self, machine_id, resources)
            .await;
    }
    async fn on_heartbeat(&self, machine_id: Uuid, report: &HarnessReport) {
        cctui_server_dispatch::routes::harness_update::on_heartbeat(self, machine_id, report).await;
    }
    async fn cancel_dispatched_workload(&self, session_id: &str) -> bool {
        cctui_server_dispatch::routes::dispatch::cancel_dispatched_workload(self, session_id).await
    }
    async fn machine_launches_find(
        &self,
        identity: &str,
        actor: Option<Uuid>,
    ) -> Result<Option<Launch>, sqlx::Error> {
        cctui_server_dispatch::machine_launches::find(&self.pool, identity, actor).await
    }
    async fn machine_launches_verdict(
        &self,
        command: Uuid,
        machine: Uuid,
        owner: Uuid,
        ok: bool,
        detail: Option<&str>,
    ) -> Result<Option<Launch>, sqlx::Error> {
        cctui_server_dispatch::machine_launches::verdict(
            &self.pool, command, machine, owner, ok, detail,
        )
        .await
    }
    async fn machine_launches_registered(
        &self,
        key: &str,
        session: &str,
        machine: Uuid,
        owner: Uuid,
    ) -> Result<(), sqlx::Error> {
        cctui_server_dispatch::machine_launches::registered(
            &self.pool, key, session, machine, owner,
        )
        .await
    }
    async fn execute_dispatch(
        &self,
        ctx: &AuthContext,
        req: DispatchRequest,
    ) -> DispatchResult<Forwarded> {
        cctui_server_dispatch::dispatch::execute_dispatch(self, ctx, req).await
    }
    fn dispatch_archive_clamp(&self, data: &mut Value) {
        cctui_server_dispatch::dispatch_archive::clamp(data);
    }
}

impl SessionsExtras for AppState {
    async fn register_conn(&self, kind: Kind, entity_id: Uuid, conn: Uuid) {
        cctui_server_extras::presence::register_conn(self, kind, entity_id, conn).await;
    }
    async fn unregister_conn(&self, kind: Kind, entity_id: Uuid, conn: Uuid) {
        cctui_server_extras::presence::unregister_conn(self, kind, entity_id, conn).await;
    }
    fn hand_off_conn(&self, kind: Kind, entity_id: Uuid, from: Uuid, to: Uuid) {
        self.presence.hand_off(kind, entity_id, from, to);
    }
    fn presence_pod(&self) -> &str {
        &self.presence.pod
    }
    async fn preview_detach_session(&self, session_id: &str) {
        self.preview.detach_session(&self.pool, session_id).await;
    }
    async fn preview_detach_machine(&self, machine_id: Uuid) {
        self.preview.detach_machine(&self.pool, machine_id).await;
    }
    async fn preview_on_frame(&self, machine_id: Uuid, daemon_user: Uuid, frame: DaemonFrameUp) {
        cctui_server_extras::preview::on_frame(self, machine_id, daemon_user, frame).await;
    }
    async fn preview_close_session(&self, session_id: &str) {
        cctui_server_extras::preview::close_session(self, session_id).await;
    }
    fn turn_ended(&self, session_id: &str) {
        cctui_server_extras::cctuiverse::turn_ended(self, session_id);
    }
    async fn langfuse_capability(&self) -> LangfuseCapability {
        match self.langfuse.as_ref() {
            Some(client) => LangfuseCapability {
                available: true,
                host: Some(client.host().to_string()),
                public_host: Some(client.public_host().to_string()),
                project_id: client.project_id().await,
            },
            None => LangfuseCapability {
                available: false,
                host: None,
                public_host: None,
                project_id: None,
            },
        }
    }
    fn plugin_setting_keys(&self, id: &str) -> Option<Vec<String>> {
        self.plugins.get(id).map(|p| p.manifest.settings.iter().map(|s| s.key.clone()).collect())
    }
    async fn plugin_host_token_reconcile(
        &self,
        user_id: Uuid,
        prev: Option<&Value>,
        next: &mut Value,
    ) -> sqlx::Result<()> {
        cctui_server_extras::plugin_host_token::reconcile(
            &self.pool,
            &self.auth_config,
            &self.plugins,
            user_id,
            prev,
            next,
        )
        .await
    }
    fn plugin_host_token_strip(&self, data: &mut Value) {
        cctui_server_extras::plugin_host_token::strip(data);
    }
    fn session_plugins(&self, settings: Option<&Value>) -> Vec<SessionPlugin> {
        cctui_server_extras::plugins::session_plugins(&self.plugins, settings)
    }
    async fn sync_or_warn(&self) {
        cctui_server_extras::plugin_store::sync_or_warn(&self.pool, &self.plugins, self).await;
    }
}

impl SessionsHost for AppState {
    fn cached_upload_caps(&self) -> UploadCaps {
        crate::routes::server_settings::cached_upload_caps(self)
    }
    async fn record_hook_flag(&self, machine: Uuid, has_hook: bool) {
        crate::routes::update_hook::record_hook_flag(&self.pool, machine, has_hook).await;
    }
}

impl SessionsNotify for AppState {
    fn emit_detached<F>(&self, owner: Option<Uuid>, build: F)
    where
        F: FnOnce(Uuid) -> NewNotification + Send + 'static,
    {
        cctui_server_notify::notify::emit_detached(&self.config, owner, build);
    }
    fn alert(&self, notice: NewNotification) {
        let pool = self.pool.clone();
        let routing = cctui_server_notify::notify::Routing::from_config(&self.config);
        let channels = cctui_server_notify::notify::Channels::from_config(&self.config);
        tokio::spawn(async move {
            cctui_server_notify::notify::emit_now(&pool, routing, &channels, notice).await;
        });
    }
    async fn on_outcome(&self, outcome: &SessionOutcome) {
        cctui_server_notify::notify::producers::on_outcome(self, outcome).await;
    }
    fn migrate_sound(&self, data: &mut Value) {
        cctui_server_notify::notify::prefs::migrate_sound(data);
    }
    async fn clamp_settings(
        &self,
        conn: &mut PgConnection,
        data: &mut Value,
    ) -> Result<(), String> {
        cctui_server_notify::notify::prefs::clamp_settings(conn, data).await
    }
    async fn user_actions_list(&self, session_id: &str) -> Result<Vec<UserAction>, AppError> {
        cctui_server_notify::routes::user_actions::list(&self.pool, session_id).await
    }
    async fn user_actions_add(
        &self,
        session_id: &str,
        req: &AddUserActionRequest,
    ) -> Result<UserActionResult, AppError> {
        cctui_server_notify::routes::user_actions::add(self, session_id, req).await
    }
    async fn user_actions_tick(
        &self,
        session_id: &str,
        id: Uuid,
        status: UserActionStatus,
        note: Option<&str>,
        by: UserActionResolver,
    ) -> Result<Ticked, AppError> {
        cctui_server_notify::routes::user_actions::tick(self, session_id, id, status, note, by)
            .await
    }
}

impl SessionsSearch for AppState {
    fn chunk_event(&self, adapter_id: &str, event_type: &str, payload: Value) -> Vec<String> {
        cctui_server_search::chunk_event(adapter_id, event_type, payload)
    }
    fn index_new(&self, rows: Vec<Chunked>) {
        cctui_server_search::index_new(&self.pool, rows);
    }
    async fn reindex(&self, ids: &[i64]) -> Result<(), sqlx::Error> {
        cctui_server_search::reindex(&self.pool, ids).await
    }
}

/// Live children of a parent, for the dispatch-side reservation.
pub struct WorkspaceChildCount;
impl ChildCount for WorkspaceChildCount {
    async fn live_child_count(
        &self,
        conn: &mut sqlx::PgConnection,
        parent_id: &str,
    ) -> Result<u32, sqlx::Error> {
        Ok(cctui_server_sessions::routes::spawn_child::live_child_count(conn, parent_id).await)
    }
}

impl ChildCount for AppState {
    async fn live_child_count(
        &self,
        conn: &mut sqlx::PgConnection,
        parent: &str,
    ) -> Result<u32, sqlx::Error> {
        WorkspaceChildCount.live_child_count(conn, parent).await
    }
}

impl ChildExecutionContext for AppState {
    fn child_state(&self) -> &impl cctui_server_dispatch::context::DispatchCore {
        &self.core
    }
    fn profiles_available(&self) -> bool {
        true
    }
    async fn remember_child_context(
        &self,
        parent: &Parent,
        key: &str,
        adapter: &str,
        cwd: Option<&str>,
        request: &SpawnChildRequest,
    ) {
        cctui_server_sessions::routes::spawn_child::remember_child_context(
            self,
            parent,
            key,
            adapter,
            cwd,
            request.agent_profile.as_deref(),
        )
        .await;
    }
    async fn deliver_child(
        &self,
        parent: &Parent,
        child: Uuid,
        spec: cctui_proto::adapter::SessionSpec,
    ) -> Result<(), AppError> {
        use cctui_server_dispatch::launch::launch_durable;
        let failed = cctui_server_kernel::commands::FailedSpawnRow {
            session_id: child.to_string(),
            machine_id: parent.machine_uuid,
            user_id: parent.user_id,
            adapter_id: spec.adapter_id.as_str().to_owned(),
            working_dir: spec.working_dir.clone().unwrap_or_default(),
            name: spec.name.clone(),
            model: spec.model.clone(),
            effort: spec.effort.clone(),
        };
        let actor = cctui_server_dispatch::child_exec::child_spawner(&self.pool, parent).await;
        let spawn = MachineSpawn {
            machine_id: parent.machine_uuid,
            command_id: child,
            token_key: child,
            spec,
            failed,
            route_session: Some(parent.session_id.clone()),
        };
        let machine = parent.machine_uuid;
        match launch_durable(&self.core, actor, spawn).await {
            Ok(()) => Ok(()),
            Err(LaunchError::Storage(error)) => Err(AppError::Db(error)),
            Err(LaunchError::Policy(detail)) => {
                Err(deny(StatusCode::BAD_REQUEST, format!("hook policy: {detail}")))
            }
            Err(LaunchError::Delivery(BusError::NoDaemon(_))) => {
                let detail =
                    cctui_server_dispatch::launch::no_daemon_detail(&self.pool, machine).await;
                Err(deny(StatusCode::SERVICE_UNAVAILABLE, detail))
            }
            Err(error) => Err(deny(
                StatusCode::SERVICE_UNAVAILABLE,
                format!("could not reach the daemon: {error}"),
            )),
        }
    }
}

impl cctui_server_dispatch::child_exec::ChildArchiveContext for AppState {
    async fn archive_child_tree(
        &self,
        _owner: Uuid,
        actor: &str,
        target: &str,
    ) -> Result<bool, sqlx::Error> {
        let outcome = cctui_server_sessions::routes::sessions::archive_one(
            self,
            target,
            false,
            cctui_proto::adapter::RemoveInitiator::Automatic,
            cctui_server_kernel::state_events::Actor::Agent(actor.to_owned()),
        )
        .await?;
        Ok(outcome != cctui_server_sessions::routes::sessions::ArchiveOutcome::SkippedPinned)
    }
    async fn archive_slots_used(&self, parent: &str) -> Result<u32, sqlx::Error> {
        Ok(cctui_server_sessions::routes::spawn_child::slots_used(self, parent).await)
    }
}

fn deny(code: StatusCode, msg: impl Into<String>) -> AppError {
    AppError::new(code, msg)
}

impl cctui_server_dispatch::session_limits::SessionLimitsContext for AppState {
    async fn limits_child_slots(
        &self,
        session_id: &str,
    ) -> Result<Option<ChildSlots>, sqlx::Error> {
        Ok(cctui_server_sessions::routes::spawn_child::child_slots(self, session_id).await)
    }
}

impl cctui_server_dispatch::launch_credentials::LaunchCredentialsContext for AppState {
    fn launch_state(
        &self,
    ) -> &(
         impl cctui_server_dispatch::context::DispatchCore
         + cctui_server_dispatch::hooks::DispatchSessions
     ) {
        &self.core
    }
    async fn default_spawn_capability(&self) -> cctui_proto::api::SpawnCapability {
        crate::routes::server_settings::spawn_default_capability(self).await
    }
}
