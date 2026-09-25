//! Daemon ↔ Server contract surface.
//!
//! Three endpoints:
//!   * `POST /api/v1/daemon/auth` — daemon confirms identity, receives
//!     `machine_id` + `user_id` so it doesn't have to know them out of band.
//!   * `GET  /api/v1/daemon/ws`   — long-lived bidirectional WS. Daemon
//!     sends [`DaemonFrameUp`]; server sends [`DaemonFrameDown`]. On
//!     connect the server emits a [`DaemonFrameDown::Reconcile`] with every
//!     known adapter, overridden by `adapters_enabled` rows.
//!   * `POST /api/v1/daemon/users/{id}/tokens` — mint a `user_tokens` row.
//!
//! Authentication: `machine_key` (Bearer) on every call; the regular
//! `auth_middleware` resolves it to `TokenRole::Machine` with
//! `machine_id` + `user_id` populated.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use cctui_proto::api::{ApiError, DaemonAuthRequest, DaemonAuthResponse};
use chrono::Utc;

use crate::state::AppState;

mod bumps;
mod connection;
mod daemon_lost;
mod decode;
mod events;
mod frames;
mod gateway_env;
mod heartbeat;
mod ingest;
mod ownership;
mod reconcile;
mod registration;
#[cfg(test)]
mod test_support;
mod tokens;

pub use connection::ws;
pub use events::truncate_end_detail;
pub use gateway_env::{TokenValidQuery, session_gateway_env, session_token_valid};
pub use reconcile::{archived_jobs, load_reconcile, load_resume_marks, load_scrub_config};
pub use tokens::{MintTokenRequest, MintTokenResponse, mint_user_token};

// ---- /api/v1/daemon/auth ----

/// Daemon presents its long-lived machine key (or a session token re-issued
/// from one). Returns the machine + owning user so the daemon can label
/// itself and avoid out-of-band configuration. v0 returns the same machine
/// key back as the session token; post-v0 may issue a short-lived JWT.
pub async fn auth(
    State(state): State<AppState>,
    Json(req): Json<DaemonAuthRequest>,
) -> Result<Json<DaemonAuthResponse>, (StatusCode, Json<ApiError>)> {
    let ctx = state.auth_config.validate(&req.machine_key).await.ok_or_else(|| {
        (StatusCode::UNAUTHORIZED, Json(ApiError { error: "invalid machine key".into() }))
    })?;
    let Some(machine_id) = ctx.machine_id else {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ApiError { error: "machine token required".into() }),
        ));
    };
    Ok(Json(DaemonAuthResponse {
        session_token: req.machine_key,
        expires_at: Utc::now() + chrono::Duration::hours(24),
        machine_id,
        user_id: ctx.user_id,
    }))
}
