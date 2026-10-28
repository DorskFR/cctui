use axum::extract::State;
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::{AuthContext, Scope, machine_token, mint_secret, sha256_hex};
use crate::error::AppError;
use crate::state::CoreState;

#[derive(Deserialize)]
pub struct EnrollRequest {
    pub hostname: String,
    #[serde(default, rename = "os")]
    pub os: Option<String>,
    #[serde(default, rename = "arch")]
    pub arch: Option<String>,
    /// Machine kind: `persistent` (a real dev-machine daemon, the
    /// default) or `ephemeral` (a dispatch/worker pod — one machine per
    /// dispatched session). Ephemeral machines are hidden from the New-session
    /// picker and reaped once they go stale. Unknown values fall back to
    /// `persistent`.
    #[serde(default)]
    pub kind: Option<String>,
}

#[derive(Serialize)]
pub struct EnrollResponse {
    pub machine_id: Uuid,
    pub machine_key: String,
    pub server_version: &'static str,
}

pub async fn enroll(
    State(state): State<CoreState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<EnrollRequest>,
) -> Result<Json<EnrollResponse>, AppError> {
    // Enrolling a machine requires the `enroll` scope. Admin holds it by
    // ceiling, so admin can enroll too. The machine is owned by the caller.
    ctx.requires(Scope::Enroll).map_err(|s| AppError::new(s, "the enroll scope is required"))?;
    let user_id = ctx.user_id;

    if req.hostname.trim().is_empty() {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "hostname required"));
    }

    // Only `ephemeral` is honoured as a non-default; anything else (including a
    // missing field, for older daemons) stays `persistent`.
    let kind = match req.kind.as_deref() {
        Some("ephemeral") => "ephemeral",
        _ => "persistent",
    };
    let (machine_id, token) = enroll_machine(&state.pool, user_id, &req.hostname, kind).await?;
    crate::events::record(
        &state.pool,
        &state.bus,
        crate::events::Event::new(
            crate::events::kind::MACHINE_ENROLLED,
            crate::events::Actor::User(user_id),
        )
        .machine(machine_id)
        .user(user_id)
        .detail(serde_json::json!({ "hostname": req.hostname, "machine_kind": kind })),
    );

    tracing::info!(
        user_id = %user_id,
        machine_id = %machine_id,
        hostname = %req.hostname,
        kind,
        "machine enrolled"
    );

    Ok(Json(EnrollResponse {
        machine_id,
        machine_key: token,
        server_version: env!("CARGO_PKG_VERSION"),
    }))
}

/// Create the machine row and its key, granted the owner's full ceiling so it
/// behaves exactly like the owner. The ceiling is read before anything is
/// written: a DB error fails the enroll instead of minting a scopeless key.
async fn enroll_machine(
    pool: &sqlx::PgPool,
    user_id: Uuid,
    hostname: &str,
    kind: &str,
) -> Result<(Uuid, String), sqlx::Error> {
    let grant = crate::store::acls::user_ceiling(pool, user_id).await?;
    let machine_id = Uuid::new_v4();
    let token = machine_token(&mint_secret());
    let key_hash = sha256_hex(&token);
    let preview = crate::auth::token_preview(&token);

    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO machines (id, user_id, name, key_hash, kind, key_preview) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(machine_id)
    .bind(user_id)
    .bind(hostname)
    .bind(&key_hash)
    .bind(kind)
    .bind(&preview)
    .execute(&mut *tx)
    .await?;

    crate::auth::register_key(
        &mut *tx,
        crate::auth::NewKey {
            user_id,
            key_hash: &key_hash,
            key_preview: Some(&preview),
            label: Some(hostname),
            kind: "machine",
            machine_id: Some(machine_id),
            dispatcher_id: None,
            expires_at: None,
            passkey_id: None,
        },
        grant,
    )
    .await?;
    tx.commit().await?;
    Ok((machine_id, token))
}

/// Remove a machine using its own credential.
///
/// `POST /api/v1/deenroll` authenticates the machine by its
/// own machine key. Used by ephemeral workers (one machine per pod) to clean
/// up on graceful exit so they don't accumulate in the machines tab. Revokes
/// and soft-deletes the row in one step (the admin purge flow requires a
/// prior revoke; self-deenroll does both). Unexpected pod deaths leave a row
/// to be cleaned via the admin revoke/delete UI.
pub async fn deenroll(
    State(state): State<CoreState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<StatusCode, AppError> {
    let machine_id = ctx
        .machine_id
        .ok_or_else(|| AppError::new(StatusCode::FORBIDDEN, "machine token required"))?;

    let old_hash: Option<(String,)> = sqlx::query_as("SELECT key_hash FROM machines WHERE id = $1")
        .bind(machine_id)
        .fetch_optional(&state.pool)
        .await?;

    let dropped = sqlx::query(
        "UPDATE machines SET revoked_at = COALESCE(revoked_at, now()), deleted_at = now() \
         WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(machine_id)
    .execute(&state.pool)
    .await?
    .rows_affected();
    if dropped > 0 {
        crate::events::record(
            &state.pool,
            &state.bus,
            crate::events::Event::new(
                crate::events::kind::MACHINE_DELETED,
                crate::events::Actor::Daemon,
            )
            .machine(machine_id)
            .user(ctx.user_id)
            .detail(serde_json::json!({ "reason": "deenrolled" })),
        );
    }

    // Auth resolves against auth_keys first; revoke the mirror row too or the
    // key keeps authenticating.
    sqlx::query(
        "UPDATE auth_keys SET revoked_at = now() WHERE machine_id = $1 AND revoked_at IS NULL",
    )
    .bind(machine_id)
    .execute(&state.pool)
    .await?;

    if let Some((hash,)) = old_hash {
        state.auth_config.purge(&hash);
    }

    tracing::info!(machine_id = %machine_id, "machine deenrolled (self)");
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn enroll_fails_on_db_error_instead_of_minting_a_scopeless_key() {
        let pool = sqlx::PgPool::connect_lazy("postgres://invalid").unwrap();
        pool.close().await;
        let err = enroll_machine(&pool, Uuid::new_v4(), "host", "persistent").await.unwrap_err();
        let status = axum::response::IntoResponse::into_response(AppError::from(err)).status();
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    }
}
