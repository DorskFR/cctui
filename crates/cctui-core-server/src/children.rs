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
