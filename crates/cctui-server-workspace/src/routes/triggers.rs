//! External trigger ingress — v0 stub.
//!
//! The route exists so consumers can target the contract surface defined by
//! the v0 platform spec. v0 returns 501 Not Implemented; post-v0 patches
//! will wire up GitHub webhooks, Slack mentions, cron, etc.

use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use cctui_proto::api::ApiError;

#[allow(clippy::unused_async)]
pub async fn ingest(Path(kind): Path<String>) -> (StatusCode, Json<ApiError>) {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(ApiError { error: format!("trigger kind '{kind}' not implemented in v0") }),
    )
}
