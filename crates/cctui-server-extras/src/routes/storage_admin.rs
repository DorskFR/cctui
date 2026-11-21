use axum::extract::State;
use axum::http::StatusCode;
use axum::{Extension, Json};
use cctui_proto::api::{
    StorageLifecycleInfo, StorageLifecyclePreview, StorageLifecycleRules, StorageOverview,
};

use crate::storage_stats;
use cctui_server_kernel::auth::{AuthContext, Scope};
use cctui_server_kernel::context::KernelContext;
use cctui_server_kernel::error::AppError;

fn admin(ctx: &AuthContext) -> Result<(), AppError> {
    ctx.requires(Scope::Admin).map_err(|s| AppError::new(s, "admin token required"))
}

fn valid(rules: StorageLifecycleRules) -> Result<StorageLifecycleRules, AppError> {
    storage_stats::validate(rules).map_err(|e| AppError::new(StatusCode::BAD_REQUEST, e))
}

pub async fn overview<S: KernelContext>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<StorageOverview>, AppError> {
    admin(&ctx)?;
    Ok(Json(storage_stats::overview(state.pool()).await?))
}

pub async fn update_lifecycle<S: KernelContext>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
    Json(rules): Json<StorageLifecycleRules>,
) -> Result<Json<StorageLifecycleInfo>, AppError> {
    admin(&ctx)?;
    storage_stats::save_rules(state.pool(), valid(rules)?, ctx.user_id).await?;
    tracing::info!(user = %ctx.user_id, ?rules, "storage lifecycle rules saved");
    Ok(Json(storage_stats::info(state.pool(), storage_stats::NAME).await?))
}

pub async fn preview_lifecycle<S: KernelContext>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
    Json(rules): Json<StorageLifecycleRules>,
) -> Result<Json<StorageLifecyclePreview>, AppError> {
    admin(&ctx)?;
    Ok(Json(storage_stats::preview(state.pool(), valid(rules)?, None).await?))
}
