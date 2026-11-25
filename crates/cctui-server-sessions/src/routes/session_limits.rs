//! Machine-authenticated usage-tool adapter for the full host.
use crate::context::SessionsContext;
use crate::error::AppError;
use axum::Json;
use axum::extract::{Path, Query, State};
pub use cctui_server_kernel::dispatch_types::{LimitsQuery, SessionLimits};

pub async fn session_limits<S: SessionsContext>(
    State(state): State<S>,
    headers: axum::http::HeaderMap,
    Path(session_id): Path<String>,
    Query(q): Query<LimitsQuery>,
) -> Result<Json<SessionLimits>, AppError> {
    let caller = crate::routes::spawn_child::machine_user(&state, &headers)
        .await
        .map_err(|(code, Json(e))| AppError::new(code, e.error))?;
    state.session_limits_inspect(caller, &session_id, &q).await.map(Json)
}
