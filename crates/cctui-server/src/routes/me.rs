//! `GET /api/v1/me` — who the presented token resolves to.
//!
//! Returns the resolved role + identity plus a non-secret preview of the
//! presented token (same shape as `token_preview`, never the full secret).

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::{Extension, Json};

pub use cctui_proto::api::me::MeResponse;

use crate::auth::{AuthContext, token_preview};
use crate::error::AppError;
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

/// `DELETE /api/v1/me/key` — revoke the credential the caller is holding, so
/// `cctui logout --revoke` leaves nothing live behind.
///
/// A machine key is refused: the daemon's credential is not a session a human
/// may end from a client, and the env admin token has no row to revoke.
pub async fn revoke_current_key(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    headers: HeaderMap,
) -> Result<StatusCode, AppError> {
    if ctx.machine_id.is_some() {
        return Err(AppError::new(StatusCode::FORBIDDEN, "a machine key cannot revoke itself"));
    }
    if ctx.key_id.is_nil() {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "the environment admin token cannot be revoked",
        ));
    }
    let mut tx = state.pool.begin().await?;
    sqlx::query("UPDATE auth_keys SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL")
        .bind(ctx.key_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE user_tokens SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL")
        .bind(ctx.key_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    // The positive-auth cache would otherwise keep the dead key working for its TTL.
    if let Some(token) = crate::auth::bearer_or_cookie(&headers) {
        state.auth_config.purge(&crate::auth::sha256_hex(&token));
    }
    tracing::info!(user_id = %ctx.user_id, key_id = %ctx.key_id, "key revoked by its holder");
    Ok(StatusCode::NO_CONTENT)
}
