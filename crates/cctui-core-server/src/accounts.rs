//! Account administration uses the same services as the full host.
use crate::host::Host;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json, Router};
use cctui_server_core::accounts::{
    AccountInfo, CreateAccount, MoveProvider, OAuthFinish, OAuthStart, OAuthStartResponse,
    ProviderInfo, ProviderSpec, SettingsCatalogQuery, UpdateAccount, UpdateProvider,
};
use cctui_server_core::auth::{AuthContext, Scope};
use cctui_server_core::error::AppError;
use uuid::Uuid;

pub fn router() -> Router<Host> {
    use axum::routing::{get, post};
    Router::new()
        .route("/api/v1/accounts", get(list_accounts).post(create_account))
        .route("/api/v1/accounts/settings-catalog", get(settings_catalog))
        .route("/api/v1/accounts/oauth/start", post(oauth_start))
        .route("/api/v1/accounts/oauth/finish", post(oauth_finish))
        .route(
            "/api/v1/accounts/{id}",
            get(get_account).patch(update_account).delete(delete_account),
        )
        .route("/api/v1/accounts/{id}/providers", post(add_provider))
        .route(
            "/api/v1/accounts/{id}/providers/{provider_id}",
            axum::routing::patch(update_provider).delete(delete_provider),
        )
        .route("/api/v1/accounts/{id}/providers/{provider_id}/move", post(move_provider))
        .route_layer(axum::middleware::from_fn(human))
}
async fn human(
    Extension(ctx): Extension<AuthContext>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Result<axum::response::Response, StatusCode> {
    if ctx.machine_id.is_some() {
        return Err(StatusCode::FORBIDDEN);
    }
    ctx.requires(Scope::Read)?;
    Ok(next.run(request).await)
}
pub async fn list_accounts(
    State(state): State<Host>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<Vec<AccountInfo>>, AppError> {
    cctui_server_core::accounts::list_accounts(&state, &ctx).await
}

pub async fn get_account(
    State(state): State<Host>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Json<AccountInfo>, AppError> {
    cctui_server_core::accounts::get_account(&state, &ctx, id).await
}

pub async fn create_account(
    State(state): State<Host>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<CreateAccount>,
) -> Result<(StatusCode, Json<AccountInfo>), AppError> {
    cctui_server_core::accounts::create_account(&state, &ctx, req).await
}

pub async fn update_account(
    State(state): State<Host>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateAccount>,
) -> Result<Json<AccountInfo>, AppError> {
    cctui_server_core::accounts::update_account(&state, &ctx, id, req).await
}

pub async fn delete_account(
    State(state): State<Host>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    cctui_server_core::accounts::delete_account(&state, &ctx, id).await
}

pub async fn add_provider(
    State(state): State<Host>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    Json(req): Json<ProviderSpec>,
) -> Result<(StatusCode, Json<ProviderInfo>), AppError> {
    cctui_server_core::accounts::add_provider(&state, &ctx, id, req).await
}

pub async fn update_provider(
    State(state): State<Host>,
    Extension(ctx): Extension<AuthContext>,
    Path((id, provider_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<UpdateProvider>,
) -> Result<Json<ProviderInfo>, AppError> {
    cctui_server_core::accounts::update_provider(&state, &ctx, (id, provider_id), req).await
}

pub async fn delete_provider(
    State(state): State<Host>,
    Extension(ctx): Extension<AuthContext>,
    Path((id, provider_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, AppError> {
    cctui_server_core::accounts::delete_provider(&state, &ctx, (id, provider_id)).await
}

pub async fn move_provider(
    State(state): State<Host>,
    Extension(ctx): Extension<AuthContext>,
    Path((id, provider_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<MoveProvider>,
) -> Result<Json<ProviderInfo>, AppError> {
    cctui_server_core::accounts::move_provider(&state, &ctx, (id, provider_id), req).await
}

pub async fn oauth_start(
    State(state): State<Host>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<OAuthStart>,
) -> Result<Json<OAuthStartResponse>, AppError> {
    cctui_server_core::accounts::oauth_start(&state, &ctx, req).await
}

pub async fn oauth_finish(
    State(state): State<Host>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<OAuthFinish>,
) -> Result<(StatusCode, Json<AccountInfo>), AppError> {
    cctui_server_core::accounts::oauth_finish(&state, &ctx, req).await
}

async fn settings_catalog(
    axum::extract::Query(q): axum::extract::Query<SettingsCatalogQuery>,
) -> Json<cctui_proto::api::accounts::SettingsCatalogResponse> {
    cctui_server_core::accounts::settings_catalog(&q)
}
