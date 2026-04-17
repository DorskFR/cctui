use axum::extract::State;
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use cctui_proto::api::ApiError;

use crate::auth::{AuthContext, machine_token, mint_secret, require_user, sha256_hex};
use crate::state::AppState;

#[derive(Deserialize)]
pub struct EnrollRequest {
    pub hostname: String,
    #[serde(default, rename = "os")]
    pub _os: Option<String>,
    #[serde(default, rename = "arch")]
    pub _arch: Option<String>,
}

#[derive(Serialize)]
pub struct EnrollResponse {
    pub machine_id: Uuid,
    pub machine_key: String,
    pub server_version: &'static str,
}

pub async fn enroll(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<EnrollRequest>,
) -> Result<Json<EnrollResponse>, (StatusCode, Json<ApiError>)> {
    let user_id = require_user(&ctx)
        .map_err(|s| (s, Json(ApiError { error: "user token required".into() })))?;

    if req.hostname.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ApiError { error: "hostname required".into() }),
        ));
    }

    let machine_id = Uuid::new_v4();
    let secret = mint_secret();
    let token = machine_token(&secret);
    let key_hash = sha256_hex(&token);

    sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, $3, $4)")
        .bind(machine_id)
        .bind(user_id)
        .bind(&req.hostname)
        .bind(&key_hash)
        .execute(&state.pool)
        .await
        .map_err(|e| {
            tracing::error!("db error: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "database error".into() }))
        })?;

    tracing::info!(
        user_id = %user_id,
        machine_id = %machine_id,
        hostname = %req.hostname,
        "machine enrolled"
    );

    Ok(Json(EnrollResponse {
        machine_id,
        machine_key: token,
        server_version: env!("CARGO_PKG_VERSION"),
    }))
}
