//! `GET /api/v1/me` — who the presented token resolves to (CCT-251).
//!
//! The webui stores a single opaque bearer and previously had no way to tell
//! whether it was the admin token, a user token, or something else — which made
//! "user token required" errors (e.g. on the OAuth account endpoints) baffling.
//! This returns the resolved role + identity plus a non-secret preview of the
//! presented token (same shape as `token_preview`, never the full secret).

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::{Extension, Json};
use serde::Serialize;
use ts_rs::TS;
use uuid::Uuid;

use crate::auth::{AuthContext, TokenRole, token_preview};
use crate::state::AppState;

#[derive(Serialize, TS)]
#[ts(export)]
pub struct MeResponse {
    /// `admin` | `user` | `machine`.
    pub role: String,
    pub user_id: Option<Uuid>,
    /// Resolved from `users.name` when the token maps to a user.
    pub user_name: Option<String>,
    pub machine_id: Option<Uuid>,
    /// Non-secret fragment of the token this request authenticated with,
    /// e.g. `cctui_u_ab1234…ef34`.
    pub token_preview: String,
}

pub async fn me(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    headers: HeaderMap,
) -> Result<Json<MeResponse>, StatusCode> {
    let role = match ctx.role {
        TokenRole::Admin => "admin",
        TokenRole::User => "user",
        TokenRole::Machine => "machine",
    };
    // The middleware already validated this header; re-read it only to build
    // the display preview (the AuthContext deliberately doesn't carry secrets).
    let preview = headers
        .get(http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(token_preview)
        .unwrap_or_default();

    let user_name = match ctx.user_id {
        Some(uid) => sqlx::query_as::<_, (String,)>("SELECT name FROM users WHERE id = $1")
            .bind(uid)
            .fetch_optional(&state.pool)
            .await
            .map_err(|e| {
                tracing::error!("db error: {e}");
                StatusCode::INTERNAL_SERVER_ERROR
            })?
            .map(|(n,)| n),
        None => None,
    };

    Ok(Json(MeResponse {
        role: role.into(),
        user_id: ctx.user_id,
        user_name,
        machine_id: ctx.machine_id,
        token_preview: preview,
    }))
}
