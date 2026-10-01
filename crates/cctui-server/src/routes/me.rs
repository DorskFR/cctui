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
    // Revoke by hash, not by id: `ctx.key_id` is the `auth_keys` row, and the
    // mirrors `resolve` falls back on key off the hash under their own ids. A
    // revoke that misses one of them leaves the credential working through the
    // legacy path.
    let Some(token) = crate::auth::bearer_or_cookie(&headers) else {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "no bearer credential to revoke"));
    };
    let hash = crate::auth::sha256_hex(&token);

    if revoke_by_hash(&state.pool, &hash).await? == 0 {
        return Err(AppError::new(StatusCode::NOT_FOUND, "this credential is already revoked"));
    }

    // The positive-auth cache would otherwise keep the dead key working for its TTL.
    state.auth_config.purge(&hash);
    tracing::info!(user_id = %ctx.user_id, key_id = %ctx.key_id, "key revoked by its holder");
    Ok(StatusCode::NO_CONTENT)
}

/// Retire every row `AuthConfig::resolve` would accept this hash from, in one
/// transaction. Returns how many rows stopped being usable.
///
/// All three are needed: `resolve` falls back from `auth_keys` to the legacy
/// tables, so revoking only the first leaves the credential working with the
/// owner's full ceiling.
pub async fn revoke_by_hash(pool: &sqlx::PgPool, hash: &str) -> Result<u64, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let keys = sqlx::query(
        "UPDATE auth_keys SET revoked_at = now() WHERE key_hash = $1 AND revoked_at IS NULL",
    )
    .bind(hash)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    let tokens = sqlx::query(
        "UPDATE user_tokens SET revoked_at = now() WHERE token_hash = $1 AND revoked_at IS NULL",
    )
    .bind(hash)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    // `users.key_hash` is NOT NULL, so a legacy single-token credential is
    // retired by rotating the column to a value no sha256 can equal, the way
    // `seed_admin` keeps the seeded row off that path.
    let legacy = sqlx::query(
        "UPDATE users SET key_hash = 'revoked-' || gen_random_uuid()::text WHERE key_hash = $1",
    )
    .bind(hash)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    tx.commit().await?;
    Ok(keys + tokens + legacy)
}

#[cfg(test)]
mod tests {
    use super::revoke_by_hash;
    use crate::auth::{AuthConfig, NewKey, Scope, register_key, sha256_hex};
    use uuid::Uuid;

    async fn test_pool(name: &str) -> Option<sqlx::PgPool> {
        let url = crate::routes::gateway::test_db_url(name)?;
        Some(
            sqlx::postgres::PgPoolOptions::new()
                .max_connections(2)
                .connect(&url)
                .await
                .expect("connect test db"),
        )
    }

    async fn seed_user(pool: &sqlx::PgPool, key_hash: &str) -> Uuid {
        let user_id = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(user_id)
            .bind(format!("revoke-test-{user_id}"))
            .bind(key_hash)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO user_acls (user_id, scope) VALUES ($1, 'read')")
            .bind(user_id)
            .execute(pool)
            .await
            .unwrap();
        user_id
    }

    /// The device-login shape: a `user_tokens` row and an `auth_keys` row for
    /// the same secret, each with its own id. Revoking must kill both, or the
    /// legacy fallback keeps authenticating the "revoked" key.
    #[tokio::test]
    async fn revoking_a_device_style_key_also_kills_its_user_tokens_mirror() {
        let name = "revoking_a_device_style_key_also_kills_its_user_tokens_mirror";
        let Some(pool) = test_pool(name).await else { return };
        let user_id = seed_user(&pool, &format!("not-a-sha-{}", Uuid::new_v4())).await;

        let token = crate::auth::user_token(&crate::auth::mint_secret());
        let hash = sha256_hex(&token);
        sqlx::query(
            "INSERT INTO user_tokens (user_id, token_hash, label, token_preview) \
             VALUES ($1, $2, 'device: test', 'prev')",
        )
        .bind(user_id)
        .bind(&hash)
        .execute(&pool)
        .await
        .unwrap();
        let key_id = register_key(
            &pool,
            NewKey {
                user_id,
                key_hash: &hash,
                key_preview: Some("prev"),
                label: Some("device: test"),
                kind: "user",
                machine_id: None,
                dispatcher_id: None,
                expires_at: None,
                passkey_id: None,
            },
            std::iter::once(Scope::Read),
        )
        .await
        .unwrap();
        assert_ne!(key_id, user_id, "the auth_keys id is its own uuid");

        let cfg = AuthConfig::new(vec![], pool.clone());
        assert!(cfg.validate(&token).await.is_some(), "the key works before revoke");

        assert!(revoke_by_hash(&pool, &hash).await.unwrap() >= 2, "both rows are retired");

        // A fresh config, so this is the database's answer and not a cache hit.
        let after = AuthConfig::new(vec![], pool.clone());
        assert!(
            after.validate(&token).await.is_none(),
            "a revoked device key must not resolve through the legacy user_tokens path"
        );
        assert_eq!(revoke_by_hash(&pool, &hash).await.unwrap(), 0, "revoking twice is a no-op");
    }

    /// A legacy `users.key_hash` credential has no `auth_keys` row of its own,
    /// and its `key_id` is the user id, so an id-keyed revoke did nothing.
    #[tokio::test]
    async fn revoking_a_legacy_users_key_hash_credential_retires_it() {
        let name = "revoking_a_legacy_users_key_hash_credential_retires_it";
        let Some(pool) = test_pool(name).await else { return };
        let token = format!("legacy-{}", Uuid::new_v4());
        let hash = sha256_hex(&token);
        let user_id = seed_user(&pool, &hash).await;

        let cfg = AuthConfig::new(vec![], pool.clone());
        let ctx = cfg.validate(&token).await.expect("the legacy key works before revoke");
        assert_eq!(ctx.key_id, user_id, "which is why an id-keyed revoke missed it");

        assert_eq!(revoke_by_hash(&pool, &hash).await.unwrap(), 1);

        let after = AuthConfig::new(vec![], pool.clone());
        assert!(after.validate(&token).await.is_none(), "the legacy key must stop resolving");
        // The user itself is untouched: this revokes a credential, not an account.
        let live: Option<chrono::DateTime<chrono::Utc>> =
            sqlx::query_scalar("SELECT revoked_at FROM users WHERE id = $1")
                .bind(user_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(live.is_none(), "revoking a key must not revoke the user");
    }
}
