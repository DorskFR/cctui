use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use uuid::Uuid;

use cctui_proto::api::presets::{LaunchPreset, LaunchPresetRequest};

use crate::auth::{AuthContext, Scope};
use crate::error::AppError;
use crate::presets::store::{self, WriteError};
use crate::presets::{ValidPreset, validate};
use crate::state::AppState;

type ApiResult<T> = Result<T, AppError>;

fn not_found() -> AppError {
    AppError::new(StatusCode::NOT_FOUND, "no preset with that id")
}

fn write_error(e: WriteError) -> AppError {
    match e {
        WriteError::Invalid(msg) => AppError::new(StatusCode::BAD_REQUEST, msg),
        WriteError::NameTaken => {
            AppError::new(StatusCode::CONFLICT, "you already have a preset with that name")
        }
        WriteError::Db(e) => e.into(),
    }
}

fn writable(ctx: &AuthContext, req: &LaunchPresetRequest) -> ApiResult<ValidPreset> {
    ctx.requires(Scope::Dispatch)
        .map_err(|code| AppError::new(code, "changing a preset needs the dispatch scope"))?;
    validate(req).map_err(|msg| AppError::new(StatusCode::BAD_REQUEST, msg))
}

pub async fn list(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> ApiResult<Json<Vec<LaunchPreset>>> {
    Ok(Json(store::list(&state.pool, ctx.owner_filter()).await?))
}

pub async fn create(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<LaunchPresetRequest>,
) -> ApiResult<(StatusCode, Json<LaunchPreset>)> {
    let preset = writable(&ctx, &req)?;
    let created = store::create(&state.pool, ctx.user_id, &preset).await.map_err(write_error)?;
    Ok((StatusCode::CREATED, Json(created)))
}

pub async fn get_one(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<LaunchPreset>> {
    store::get(&state.pool, ctx.owner_filter(), id).await?.map(Json).ok_or_else(not_found)
}

pub async fn update(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    Json(req): Json<LaunchPresetRequest>,
) -> ApiResult<Json<LaunchPreset>> {
    let preset = writable(&ctx, &req)?;
    store::update(&state.pool, ctx.owner_filter(), id, &preset)
        .await
        .map_err(write_error)?
        .map(Json)
        .ok_or_else(not_found)
}

pub async fn delete(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    ctx.requires(Scope::Dispatch)
        .map_err(|code| AppError::new(code, "deleting a preset needs the dispatch scope"))?;
    if store::delete(&state.pool, ctx.owner_filter(), id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(not_found())
    }
}

#[cfg(test)]
#[path = "presets_tests.rs"]
mod tests;
