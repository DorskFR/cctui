//! Machine-authenticated usage reporting for execution sessions.
use crate::host::Host;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use cctui_server_core as core;
use core::child_policy::ChildSlots;
use core::error::AppError;
use core::session_limits::{LimitsQuery, SessionLimits};
impl core::session_limits::SessionLimitsContext for Host {
    async fn limits_child_slots(
        &self,
        session_id: &str,
    ) -> Result<Option<ChildSlots>, sqlx::Error> {
        core::child_policy::child_slots(self, session_id).await
    }
}
pub async fn session_limits(
    State(host): State<Host>,
    headers: HeaderMap,
    Path(session): Path<String>,
    Query(q): Query<LimitsQuery>,
) -> Result<Json<SessionLimits>, AppError> {
    let (user, machine) = crate::launch_credentials::machine(&host, &headers)
        .await
        .map_err(|code| AppError::new(code, "machine key required"))?;
    if !core::launch_credentials::gateway_env_allowed(&host.pool, user, machine, &session).await? {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "session is unavailable for this machine",
        ));
    }
    core::session_limits::inspect(&host, user, &session, &q).await.map(Json)
}
