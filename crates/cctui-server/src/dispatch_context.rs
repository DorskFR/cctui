//! The host's implementation of the dispatch context and hooks.
use std::sync::Arc;

use cctui_proto::models::MachineLiveness;
use cctui_server_dispatch::context::DispatchCore;
use cctui_server_dispatch::dispatchers::Registry;
use cctui_server_kernel::auth::AuthConfig;
use cctui_server_kernel::bus::{Bus, BusError};
use cctui_server_kernel::commands::{CommandWaiters, PendingCommand};
use dashmap::DashMap;
use sqlx::PgPool;
use uuid::Uuid;

use cctui_proto::api::DispatchRequest;
use cctui_server_dispatch::dispatch::{DispatchContext, DispatchEvent, DispatchNotification};
use cctui_server_dispatch::routes::dispatch::payload_adapter;
use cctui_server_kernel::auth::AuthContext;

use crate::state::AppState;
use cctui_server_notify::notify::{NewNotification, kind};

impl DispatchCore for AppState {
    fn pool(&self) -> &PgPool {
        &self.core.pool
    }
    fn bus(&self) -> &Bus {
        &self.core.bus
    }
    fn auth_config(&self) -> &AuthConfig {
        &self.core.auth_config
    }
    fn http_client(&self) -> &reqwest::Client {
        &self.core.http_client
    }
    fn pending_commands(&self) -> &DashMap<Uuid, PendingCommand> {
        &self.core.pending_commands
    }
    fn command_waiters(&self) -> &CommandWaiters {
        &self.core.command_waiters
    }
    fn spawn_capabilities(&self) -> &DashMap<String, cctui_proto::api::SpawnCapability> {
        &self.core.spawn_capabilities
    }
    fn session_usd_budgets(&self) -> &DashMap<String, f64> {
        &self.core.session_usd_budgets
    }
    fn dispatcher_registry(&self) -> &Arc<Registry> {
        &self.core.dispatchers
    }
    fn dispatcher_liveness(&self) -> &DashMap<Uuid, MachineLiveness> {
        &self.core.dispatcher_liveness
    }
    fn machine_liveness(&self) -> &DashMap<Uuid, MachineLiveness> {
        &self.core.machine_liveness
    }
}

crate::impl_dispatch_account_resolution!(AppState);
crate::impl_dispatch_mint!(AppState);
crate::impl_dispatch_usage!(AppState);
crate::impl_dispatch_account_store!(AppState, pool);

impl cctui_server_dispatch::targets::dispatcher::DispatcherContext for AppState {
    async fn register_dispatcher_presence(&self, id: Uuid) {
        cctui_server_extras::presence::register(
            self,
            cctui_server_extras::presence::Kind::Dispatcher,
            id,
        )
        .await;
    }
    async fn unregister_dispatcher_presence(&self, id: Uuid) {
        cctui_server_extras::presence::unregister(
            self,
            cctui_server_extras::presence::Kind::Dispatcher,
            id,
        )
        .await;
    }
}

impl cctui_server_dispatch::targets::manifest::DistributionContext for AppState {
    fn distribution_url(&self) -> &str {
        &self.config.external_url
    }
    fn github_token(&self) -> Option<Arc<str>> {
        self.config.github_token.as_ref().map(cctui_secrets::Secret::get)
    }
}

impl cctui_server_dispatch::hooks::DispatchPresence for AppState {
    fn presence_pod(&self) -> &str {
        &self.presence.pod
    }
}

impl cctui_server_dispatch::hooks::DispatchSessionArchive for AppState {
    async fn archive_one(
        &self,
        session_id: &str,
        force: bool,
        initiator: cctui_proto::adapter::RemoveInitiator,
        actor: cctui_server_kernel::events::Actor,
    ) -> Result<cctui_server_kernel::sessions_types::ArchiveOutcome, sqlx::Error> {
        cctui_server_sessions::routes::sessions::archive_one(
            self, session_id, force, initiator, actor,
        )
        .await
    }
    async fn any_viewed(&self, sessions: &[String]) -> Result<bool, sqlx::Error> {
        cctui_server_sessions::session_viewers::any_viewed(&self.pool, sessions).await
    }
    async fn prompting(&self, tree: &[String]) -> bool {
        let prompts = self.permission_store.read().await;
        tree.iter().any(|s| prompts.pending_ask(s).is_some() || prompts.pending_plan(s).is_some())
            || prompts.list_pending().iter().any(|p| tree.contains(&p.session_id))
    }
}

impl cctui_server_dispatch::hooks::DispatchSessions for AppState {
    async fn push_reconcile(&self, machine_id: Uuid) -> Result<(), BusError> {
        cctui_server_sessions::targets::daemon::reconcile::push_reconcile(&self.core, machine_id)
            .await
    }
    async fn enabled_adapter_ids(&self, machine_id: Uuid) -> sqlx::Result<Vec<String>> {
        cctui_server_sessions::targets::daemon::reconcile::enabled_adapter_ids(
            &self.core, machine_id,
        )
        .await
    }
}

/// Received and forwarded are `dispatch_info`, off by default.
const fn notice_kind(event: DispatchEvent) -> &'static str {
    match event {
        DispatchEvent::Received | DispatchEvent::Forwarded => kind::DISPATCH_INFO,
        DispatchEvent::Failed => kind::DISPATCH_FAILED,
    }
}

/// A registered session opens itself; otherwise the subject names the failure record.
pub fn dispatch_notice(owner: uuid::Uuid, notice: DispatchNotification) -> NewNotification {
    let (severity, tags, collapse) = match notice.event {
        DispatchEvent::Received => ("info", "inbox_tray", "dispatch"),
        DispatchEvent::Forwarded => ("info", "white_check_mark", "dispatch"),
        DispatchEvent::Failed => ("critical", "rotating_light", kind::DISPATCH_FAILED),
    };
    NewNotification {
        severity,
        tags: Some(tags.to_owned()),
        collapse_key: Some(format!("{collapse}:{}", notice.session_id)),
        session_id: notice.registered.then(|| notice.session_id.clone()),
        subject_kind: Some(cctui_server_notify::notify::producers::DISPATCH_SUBJECT.to_owned()),
        subject_id: Some(notice.session_id),
        body: Some(notice.message),
        ..NewNotification::new(owner, notice_kind(notice.event), notice.title)
    }
}

impl DispatchContext for AppState {
    fn dispatch_bus(&self) -> &Bus {
        &self.bus
    }
    fn dispatchers(&self) -> &Registry {
        &self.dispatchers
    }
    fn notify(&self, notice: DispatchNotification) {
        if !cctui_server_notify::notify::is_recorded(notice_kind(notice.event)) {
            return;
        }
        let owner = notice.owner;
        cctui_server_notify::notify::emit_detached_on(
            self.pool.clone(),
            &self.config,
            owner,
            move |owner| dispatch_notice(owner, notice),
        );
    }
    fn cache_spawn_capability(&self, session: &str, cap: cctui_proto::api::SpawnCapability) {
        self.spawn_capabilities.insert(session.to_owned(), cap);
    }
    async fn spawn_default_capability(&self) -> cctui_proto::api::SpawnCapability {
        crate::routes::server_settings::spawn_default_capability(self).await
    }
    async fn register_completion_webhook(
        &self,
        ctx: &AuthContext,
        req: &DispatchRequest,
        session: &str,
    ) {
        register_completion_webhook(self, ctx, req, session).await;
    }
    async fn remember_dispatch_context(
        &self,
        real_user: Option<uuid::Uuid>,
        req: &DispatchRequest,
        session: &str,
    ) {
        if let Some(uid) = real_user {
            if context_record_is_callers(&self.pool, session, uid).await {
                let scope = dispatch_context_scope(uid, &req.dispatcher, &req.payload);
                if self.config.features.contains(cctui_proto::features::Feature::Context) {
                    cctui_server_sessions::routes::context::remember_intent(
                        &self.pool,
                        session,
                        &[],
                        &scope,
                    )
                    .await;
                } else {
                    cctui_server_sessions::routes::context::ownership::record(
                        &self.pool, session, &scope,
                    )
                    .await;
                }
            } else {
                tracing::warn!(
                    %session,
                    "dispatch names another user's session; its context set is left alone"
                );
            }
        }
    }
}

/// Register a server-emitted completion webhook when the caller supplied
/// `notify_url`. The server fires it once the dispatched session reaches a
/// terminal state — covering crash cases the worker's `REPLY_URL` exit trap can
/// miss. Scoped to a real owning user (admin-token dispatches carry no owner,
/// so they keep the `REPLY_URL` trap only). Best-effort: a registration failure
/// never blocks the dispatch. The `task_id` echoed back is the dispatch
/// payload's `task_id` if present, else the session id.
async fn register_completion_webhook(
    state: &AppState,
    ctx: &AuthContext,
    req: &DispatchRequest,
    session_id: &str,
) {
    if let (Some(uid), Some(notify_url)) =
        (ctx.owner_filter(), req.notify_url.as_deref().filter(|u| !u.trim().is_empty()))
    {
        let task_id =
            req.payload.get("task_id").and_then(serde_json::Value::as_str).unwrap_or(session_id);
        cctui_server_dispatch::webhook::register(
            state,
            session_id,
            uid,
            notify_url,
            req.notify_secret.as_deref().filter(|s| !s.trim().is_empty()),
            task_id,
        )
        .await;
    }
}

impl cctui_server_dispatch::hooks::DispatchNotify for AppState {
    async fn resolve_notices(&self, user_id: uuid::Uuid, session_ids: &[String]) {
        let closed = cctui_server_notify::notify::producers::dispatch_dismissed(
            &self.pool,
            user_id,
            session_ids,
        )
        .await;
        if let Err(e) = closed {
            tracing::warn!(?session_ids, "failed dispatch notices not resolved: {e}");
        }
    }
}

/// A dispatch may name an existing session id as its retry target, so the
/// context set it records must not overwrite one that belongs to someone else:
/// neither a set already keyed by that id, nor a registered session, may be
/// another user's. A lookup failure keeps the set untouched.
pub async fn context_record_is_callers(
    pool: &sqlx::PgPool,
    session_id: &str,
    uid: uuid::Uuid,
) -> bool {
    let foreign: Result<bool, sqlx::Error> = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM session_context \
                        WHERE spawn_key = $1 AND scope->>'user_id' <> $2) \
            OR EXISTS (SELECT 1 FROM sessions \
                        WHERE id = $1 AND user_id IS NOT NULL AND user_id <> $3)",
    )
    .bind(session_id)
    .bind(uid.to_string())
    .bind(uid)
    .fetch_one(pool)
    .await;
    match foreign {
        Ok(foreign) => !foreign,
        Err(e) => {
            tracing::warn!(%session_id, error = %e, "context ownership lookup failed");
            false
        }
    }
}

/// The coordinates a dispatched worker's rules resolve against: only the
/// items its owner opted in to dispatches reach it. No human watches a worker,
/// so everything it saves through `CctuiContext` waits for approval.
pub fn dispatch_context_scope(
    uid: uuid::Uuid,
    dispatcher: &str,
    payload: &serde_json::Value,
) -> cctui_server_sessions::routes::context::StoredScope {
    let text = |key: &str| {
        payload
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    cctui_server_sessions::routes::context::StoredScope {
        user_id: uid,
        coords: cctui_server_sessions::routes::context::SpawnScope {
            machine_id: None,
            working_dir: text("working_dir").or_else(|| text("cwd")),
            label_ids: Vec::new(),
            adapter: Some(payload_adapter(payload).unwrap_or("claude-code").to_owned()),
            machine_owner: None,
            dispatch: Some(cctui_server_sessions::routes::context::DispatchCoords {
                owner: uid,
                dispatcher: Some(dispatcher.trim().to_owned()).filter(|d| !d.is_empty()),
                profile: text("profile"),
            }),
        },
        dispatched: true,
    }
}
