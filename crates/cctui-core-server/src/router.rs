//! HTTP assembly for the automation distribution.
use crate::host::Host;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::routing::{any, get, post};
use axum::{Extension, Json, Router};
use cctui_server_core as core;
use core::auth::{AuthContext, auth_middleware};
use core::gateway::Family;
use core::gateway::proxy::passthrough;
use core::identity::{admin, me};

pub fn router(host: Host) -> Router {
    let authenticated = Router::new()
        .route("/api/v1/admin/users", get(admin::list_users).post(admin::create_user))
        .route(
            "/api/v1/admin/users/{id}",
            axum::routing::patch(admin::update_user).delete(admin::purge_user),
        )
        .route("/api/v1/admin/users/{id}/revoke", post(admin::revoke_user))
        .route("/api/v1/users/{id}/keys", get(admin::list_user_keys).post(admin::mint_user_key))
        .route("/api/v1/users/{id}/keys/{kid}", axum::routing::delete(admin::revoke_user_key))
        .route("/api/v1/me", get(me::me))
        .route("/api/v1/me/key", axum::routing::delete(me::revoke_current_key))
        .route("/api/v1/enroll", post(core::targets::enrollment::enroll))
        .route("/api/v1/deenroll", post(core::targets::enrollment::deenroll))
        .route("/api/v1/sessions/dispatch", post(dispatch))
        .route("/api/v1/sessions/{id}/result", get(crate::runs::result))
        .route("/api/v1/sessions/{id}/kill", post(crate::runs::kill))
        .layer(axum::middleware::from_fn(auth_middleware))
        .layer(Extension(host.auth_config.clone()));
    Router::new()
        .merge(authenticated)
        .route("/api/v1/daemon/auth", post(crate::daemon::auth))
        .route("/api/v1/daemon/ws", get(crate::daemon::ws))
        .route("/api/v1/version", get(version))
        .route("/api/v1/capabilities", get(capabilities))
        .route("/health", get(|| async { "ok" }))
        .route("/ready", get(ready))
        .route("/gateway/anthropic/{*path}", any(anthropic))
        .route("/gateway/openai/{*path}", any(openai))
        .route("/gateway/fireworks/{*path}", any(fireworks))
        .route("/gateway/google/{*path}", any(google))
        .with_state(host)
}

async fn ready(State(host): State<Host>) -> StatusCode {
    if sqlx::query("SELECT 1").execute(&host.pool).await.is_ok() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    }
}
async fn dispatch(
    State(host): State<Host>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<cctui_proto::api::DispatchRequest>,
) -> core::dispatch::DispatchResult<core::dispatch::Forwarded> {
    // Until completion delivery is installed, refuse a requested callback before launching.
    if req.notify_url.is_some() || req.notify_secret.is_some() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(cctui_proto::api::ApiError {
                error: "completion notifications are not yet available in the automation beta"
                    .into(),
            }),
        ));
    }
    core::dispatch::execute_dispatch(&host, &ctx, req).await
}
async fn anthropic(
    State(host): State<Host>,
    req: Request,
) -> Result<axum::response::Response, StatusCode> {
    passthrough(
        host,
        req,
        Family::Anthropic,
        "/gateway/anthropic",
        &core::gateway::anthropic_upstream(),
    )
    .await
}
async fn openai(
    State(host): State<Host>,
    req: Request,
) -> Result<axum::response::Response, StatusCode> {
    passthrough(host, req, Family::Openai, "/gateway/openai", &core::gateway::openai_upstream())
        .await
}
async fn fireworks(
    State(host): State<Host>,
    req: Request,
) -> Result<axum::response::Response, StatusCode> {
    passthrough(
        host,
        req,
        Family::Fireworks,
        "/gateway/fireworks",
        &core::gateway::fireworks_upstream(),
    )
    .await
}
async fn google(
    State(host): State<Host>,
    req: Request,
) -> Result<axum::response::Response, StatusCode> {
    passthrough(host, req, Family::Google, "/gateway/google", &core::gateway::google_upstream())
        .await
}

async fn version() -> Json<cctui_proto::api::version::VersionInfo> {
    let hash = env!("CCTUI_GIT_HASH");
    Json(cctui_proto::api::version::VersionInfo {
        version: env!("CARGO_PKG_VERSION"),
        git_hash: hash,
        repo_url: "https://github.com/DorskFR/cctui",
        commit_url: format!("https://github.com/DorskFR/cctui/commit/{hash}"),
        latest_version: None,
        latest_url: None,
        instance_name: None,
        self_update_ready: false,
        self_update_hook: false,
        upload_caps: cctui_proto::api::uploads::UploadCaps::default(),
    })
}
async fn capabilities() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "langfuse": {"available":false,"host":null,"public_host":null,"project_id":null},
        "distribution": "core",
        "features": ["identity", "enrollment", "dispatch", "gateway", "run_results", "cancellation"],
        "workspace": false,
        "experimental": true,
    }))
}
