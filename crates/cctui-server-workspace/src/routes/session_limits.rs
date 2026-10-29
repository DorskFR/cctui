//! Machine-authenticated usage-tool adapter for the full host.
use crate::error::AppError;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, Query, State};
use cctui_server_core::child_policy::ChildSlots;
pub use cctui_server_core::session_limits::{LimitsQuery, SessionLimits};

impl cctui_server_core::session_limits::SessionLimitsContext for AppState {
    async fn limits_child_slots(
        &self,
        session_id: &str,
    ) -> Result<Option<ChildSlots>, sqlx::Error> {
        Ok(crate::routes::spawn_child::child_slots(self, session_id).await)
    }
}

pub async fn session_limits(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(session_id): Path<String>,
    Query(q): Query<LimitsQuery>,
) -> Result<Json<SessionLimits>, AppError> {
    let caller = crate::routes::spawn_child::machine_user(&state, &headers)
        .await
        .map_err(|(code, Json(e))| AppError::new(code, e.error))?;
    cctui_server_core::session_limits::inspect(&state, caller, &session_id, &q).await.map(Json)
}
