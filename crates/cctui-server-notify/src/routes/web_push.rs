use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;

pub use cctui_proto::api::notifications::VapidKey;

use cctui_server_kernel::context::KernelContext;
use cctui_server_kernel::error::AppError;

pub async fn vapid_key<S: KernelContext>(
    State(state): State<S>,
) -> Result<Json<VapidKey>, AppError> {
    let key = crate::notify::web_push::vapid(state.pool()).await.map_err(|e| {
        tracing::warn!("VAPID key unavailable: {e}");
        AppError::new(StatusCode::SERVICE_UNAVAILABLE, "web push is unavailable on this server")
    })?;
    Ok(Json(VapidKey { public_key: key.public_key() }))
}
