//! `GET /api/v1/sessions/{id}/langfuse` — per-session cost/usage rollup proxied
//! from Langfuse. Keys stay server-side; the browser never talks to
//! Langfuse. Ownership is enforced by the `sess_read` authz layer; the response
//! is cached ~60s in the [`crate::langfuse::LangfuseClient`] to spare upstream.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;

use crate::error::AppError;
use crate::langfuse::LangfuseSessionUsage;
use crate::state::AppState;

pub async fn session_langfuse(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<LangfuseSessionUsage>, AppError> {
    let Some(client) = state.langfuse.as_ref() else {
        return Err(AppError::new(StatusCode::NOT_FOUND, "langfuse not configured"));
    };
    match client.session_usage(&session_id).await {
        Ok(usage) => Ok(Json(usage)),
        Err(e) => {
            tracing::debug!("langfuse session usage failed: {e}");
            Err(AppError::new(StatusCode::BAD_GATEWAY, "langfuse read failed"))
        }
    }
}
