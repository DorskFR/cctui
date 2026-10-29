//! Machine-authenticated child execution for the automation host.
use crate::host::Host;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use cctui_proto::adapter::SessionSpec;
use cctui_proto::api::{SpawnChildRequest, SpawnChildResponse};
use cctui_server_core as core;
use core::child_exec::{ChildExecutionContext, Parent};
use core::child_policy::{ChildCount, ExecutionChildCount};
use core::error::AppError;
use uuid::Uuid;

pub async fn spawn_child(
    State(host): State<Host>,
    headers: HeaderMap,
    Path(parent): Path<String>,
    Json(req): Json<SpawnChildRequest>,
) -> Result<Json<SpawnChildResponse>, AppError> {
    let (user, machine) = crate::launch_credentials::machine(&host, &headers)
        .await
        .map_err(|code| AppError::new(code, "machine key required"))?;
    let _guard = host.lifecycle.lock().await;
    require_unstopped(&host, &parent, user, machine).await?;
    if !core::launch_credentials::gateway_env_allowed(&host.pool, user, machine, &parent).await? {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "parent session is unavailable for this machine",
        ));
    }
    core::child_exec::spawn_child(&host, user, &parent, &req).await.map(Json)
}
impl ChildCount for Host {
    async fn live_child_count(
        &self,
        conn: &mut sqlx::PgConnection,
        parent: &str,
    ) -> Result<u32, sqlx::Error> {
        ExecutionChildCount.live_child_count(conn, parent).await
    }
}
impl ChildExecutionContext for Host {
    fn child_state(&self) -> &core::state::CoreState {
        &self.core
    }
    async fn remember_child_context(
        &self,
        parent: &Parent,
        key: &str,
        _adapter: &str,
        _cwd: Option<&str>,
        request: &SpawnChildRequest,
    ) {
        let spawner = core::child_exec::child_spawner(&self.pool, parent).await;
        core::machine_policy::remember_spawner(&self.pool, key, spawner, request.name.as_deref())
            .await;
    }
    async fn deliver_child(
        &self,
        parent: &Parent,
        child: Uuid,
        spec: SessionSpec,
    ) -> Result<(), AppError> {
        let key = child.to_string();
        let budget = self.spawn_capabilities.get(&key).and_then(|cap| cap.max_budget_usd);
        if let Some(budget) = budget {
            self.session_usd_budgets.insert(key.clone(), budget);
        }
        let failed = core::commands::FailedSpawnRow {
            session_id: key.clone(),
            machine_id: parent.machine_uuid,
            user_id: parent.user_id,
            adapter_id: spec.adapter_id.as_str().to_owned(),
            working_dir: spec.working_dir.clone().unwrap_or_default(),
            name: spec.name.clone(),
            model: spec.model.clone(),
            effort: spec.effort.clone(),
        };
        let actor = core::child_exec::child_spawner(&self.pool, parent).await;
        let result = core::launch::launch_durable(
            &self.core,
            actor,
            core::launch::MachineSpawn {
                machine_id: parent.machine_uuid,
                command_id: child,
                token_key: child,
                spec,
                failed,
            },
        )
        .await;
        if result.is_err() {
            self.session_usd_budgets.remove(&key);
        }
        result.map_err(|error| match error {
            core::launch::LaunchError::Storage(error) => AppError::Db(error),
            error => AppError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                format!("could not reach the daemon: {error}"),
            ),
        })
    }
}

/// Relay a follow-up from the authenticated parent's machine.
pub async fn message_child(
    State(host): State<Host>,
    headers: HeaderMap,
    Path(parent): Path<String>,
    Json(req): Json<cctui_proto::api::MessageChildRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let (user, machine) = crate::launch_credentials::machine(&host, &headers)
        .await
        .map_err(|code| AppError::new(code, "machine key required"))?;
    let _guard = host.lifecycle.lock().await;
    require_unstopped(&host, &parent, user, machine).await?;
    require_unstopped(&host, req.session_id.trim(), user, machine).await?;
    if !core::launch_credentials::gateway_env_allowed(&host.pool, user, machine, &parent).await? {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "parent session is unavailable for this machine",
        ));
    }
    core::child_exec::message_child(&host, user, &parent, &req).await?;
    Ok(Json(serde_json::json!({})))
}

/// Deliver the authenticated child's completion to its live parent.
pub async fn child_completion(
    State(host): State<Host>,
    headers: HeaderMap,
    Path(child): Path<String>,
    Json(req): Json<core::child_exec::ChildCompletion>,
) -> Result<Json<serde_json::Value>, AppError> {
    let (user, machine) = crate::launch_credentials::machine(&host, &headers)
        .await
        .map_err(|code| AppError::new(code, "machine key required"))?;
    if !core::launch_credentials::gateway_env_allowed(&host.pool, user, machine, &child).await? {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "child session is unavailable for this machine",
        ));
    }
    let parent = core::child_exec::deliver_completion(&host, user, &child, &req.text).await?;
    Ok(Json(serde_json::json!({"delivered_to":parent})))
}

/// Archive a descendant without workspace journals or registries.
pub async fn archive_child(
    State(host): State<Host>,
    headers: HeaderMap,
    Path(parent): Path<String>,
    Json(req): Json<cctui_proto::api::ArchiveChildRequest>,
) -> Result<Json<cctui_proto::api::ArchiveChildResponse>, AppError> {
    let (user, machine) = crate::launch_credentials::machine(&host, &headers)
        .await
        .map_err(|code| AppError::new(code, "machine key required"))?;
    if !core::launch_credentials::gateway_env_allowed(&host.pool, user, machine, &parent).await? {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "parent session is unavailable for this machine",
        ));
    }
    core::child_exec::archive_descendant(&host, user, &parent, req.session_id.trim())
        .await
        .map(Json)
}
impl core::child_exec::ChildArchiveContext for Host {
    async fn archive_child_tree(
        &self,
        owner: Uuid,
        actor: &str,
        target: &str,
    ) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let pinned: Option<bool> =
            sqlx::query_scalar("SELECT pinned FROM sessions WHERE id=$1 AND user_id=$2 FOR UPDATE")
                .bind(target)
                .bind(owner)
                .fetch_optional(&mut *tx)
                .await?;
        if pinned != Some(false) {
            return Ok(false);
        }
        let children = core::store::sessions::descendants(&mut *tx, target).await?;
        let mut ids: Vec<String> = children.iter().map(|child| child.id.clone()).collect();
        ids.push(target.to_owned());
        let archived: Vec<String> = sqlx::query_scalar("UPDATE sessions SET status='archived',tempo=NULL,agent_state=NULL,activity=NULL,soft_limit_reason=NULL,archived_by='automatic' WHERE id=ANY($1) AND user_id=$2 AND NOT pinned RETURNING id")
            .bind(&ids).bind(owner).fetch_all(&mut *tx).await?;
        tx.commit().await?;
        for id in &archived {
            core::session_credentials::revoke_session_tokens(self, id).await;
            self.bus.deregister_session_stream(id);
        }
        let mut jobs = core::store::sessions::job_children(&children, &archived);
        if archived.iter().any(|id| id == target) {
            jobs.push(target);
        }
        for id in jobs {
            if let Err(error) = core::bus::dispatch_session(
                &self.pool,
                &self.bus,
                id,
                cctui_proto::adapter::AdapterCommand::Remove {
                    local_id: id.to_owned(),
                    command_id: None,
                    initiator: cctui_proto::adapter::RemoveInitiator::Automatic,
                },
            )
            .await
            {
                tracing::warn!(session=%id, %error, "archive persisted; worker removal could not be delivered");
            }
        }
        core::events::record(&self.pool, &self.bus,
            core::events::Event::new(core::events::kind::SESSION_ARCHIVED, core::events::Actor::Agent(actor.to_owned()))
                .session(target).detail(serde_json::json!({"children":archived.len().saturating_sub(1),"initiator":"automatic"})));
        Ok(true)
    }
    async fn archive_slots_used(&self, parent: &str) -> Result<u32, sqlx::Error> {
        let mut conn = self.pool.acquire().await?;
        let live = self.live_child_count(&mut conn, parent).await?;
        let pending = core::child_policy::pending_child_count(&mut *conn, parent).await?;
        Ok(live.saturating_add(pending))
    }
}

async fn require_unstopped(
    host: &Host,
    session: &str,
    user: Uuid,
    machine: Uuid,
) -> Result<(), AppError> {
    if crate::termination::stopped(host, session, user, machine).await? {
        return Err(AppError::new(StatusCode::GONE, "execution was stopped"));
    }
    Ok(())
}
