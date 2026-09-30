//! `GET /api/v1/me` — who the presented token resolves to.
//!
//! Returns the resolved role + identity plus a non-secret preview of the
//! presented token (same shape as `token_preview`, never the full secret).

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::{Extension, Json};

pub use cctui_proto::api::me::MeResponse;

use crate::auth::{AuthContext, token_preview};
use crate::state::AppState;

pub async fn me(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    headers: HeaderMap,
) -> Result<Json<MeResponse>, StatusCode> {
    let role = if ctx.is_admin() {
        "admin"
    } else if ctx.machine_id.is_some() {
        "machine"
    } else {
        "user"
    };
    // The middleware already validated this header; re-read it only to build
    // the display preview (the AuthContext deliberately doesn't carry secrets).
    let preview = headers
        .get(http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(token_preview)
        .unwrap_or_default();

    let user_name = sqlx::query_as::<_, (String,)>("SELECT name FROM users WHERE id = $1")
        .bind(ctx.user_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(|e| {
            tracing::error!("db error: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .map(|(n,)| n);

    Ok(Json(MeResponse {
        role: role.into(),
        user_id: Some(ctx.user_id),
        user_name,
        machine_id: ctx.machine_id,
        scopes: ctx.scopes.iter().map(std::string::ToString::to_string).collect(),
        token_preview: preview,
    }))
}
