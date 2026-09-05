//! content-addressed blob store for oversized embedded attachments.
//!
//! `PUT /api/v1/daemon/blobs/{hash}` — the daemon uploads raw bytes it extracted
//! from a transcript payload, keyed by their sha256. Machine-key Bearer
//! self-auth (like the sibling `daemon/…` endpoints). Idempotent: a re-PUT of an
//! already-stored hash is a cheap 200. The body's sha256 must equal the path
//! hash (400 otherwise) so the store stays honestly content-addressed.
//!
//! `GET /api/v1/sessions/{id}/blobs/{hash}` — a consumer resolves a
//! `{type:"cctui-blob", blob_id}` reference. Session-read authz (enforced by the
//! `api_router` layer via `{id}`) + the same-origin cookie, like the
//! image GET. The 256-bit content hash is itself an unguessable capability.

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};

use crate::state::AppState;

/// A single blob may be several MB (full-resolution screenshots); cap it well
/// above the per-image limit.
pub const MAX_BLOB_BYTES: usize = 32 * 1024 * 1024;

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

async fn require_machine(state: &AppState, headers: &header::HeaderMap) -> Result<(), StatusCode> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let ctx = state.auth_config.validate(token).await.ok_or(StatusCode::UNAUTHORIZED)?;
    if ctx.machine_id.is_none() {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}

pub async fn put_blob(
    State(state): State<AppState>,
    headers: header::HeaderMap,
    Path(hash): Path<String>,
    body: Bytes,
) -> Result<StatusCode, StatusCode> {
    require_machine(&state, &headers).await?;

    if !is_sha256_hex(&hash) {
        return Err(StatusCode::BAD_REQUEST);
    }
    if body.len() > MAX_BLOB_BYTES {
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }
    if hex::encode(Sha256::digest(&body)) != hash {
        return Err(StatusCode::BAD_REQUEST);
    }

    let media_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty() && *v != "application/octet-stream");

    match store_blob(&state.pool, &body, media_type).await {
        Ok(StoredBlob { created: true, .. }) => Ok(StatusCode::CREATED),
        Ok(StoredBlob { created: false, .. }) => Ok(StatusCode::OK),
        Err(e) => {
            tracing::error!("blob store: {e}");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

pub struct StoredBlob {
    pub hash: String,
    pub created: bool,
}

/// Idempotent: an existing hash is left as is (`created: false`).
pub async fn store_blob(
    pool: &sqlx::PgPool,
    bytes: &[u8],
    media_type: Option<&str>,
) -> Result<StoredBlob, sqlx::Error> {
    let hash = hex::encode(Sha256::digest(bytes));
    let byte_len = i64::try_from(bytes.len()).unwrap_or(i64::MAX);
    let rows = sqlx::query(
        "INSERT INTO daemon_blobs (hash, media_type, byte_len, bytes) VALUES ($1, $2, $3, $4) \
         ON CONFLICT (hash) DO NOTHING",
    )
    .bind(&hash)
    .bind(media_type)
    .bind(byte_len)
    .bind(bytes)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(StoredBlob { hash, created: rows == 1 })
}

pub async fn get_blob(
    State(state): State<AppState>,
    Path((_session_id, hash)): Path<(String, String)>,
) -> Result<Response, StatusCode> {
    if !is_sha256_hex(&hash) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let row: Option<(Option<String>, Vec<u8>)> =
        sqlx::query_as("SELECT media_type, bytes FROM daemon_blobs WHERE hash = $1")
            .bind(&hash)
            .fetch_optional(&state.pool)
            .await
            .map_err(|e| {
                tracing::error!("blob get: {e}");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
    let (media_type, bytes) = row.ok_or(StatusCode::NOT_FOUND)?;

    let mut resp = Response::new(axum::body::Body::from(bytes));
    let ct = media_type
        .as_deref()
        .and_then(|m| m.parse().ok())
        .unwrap_or_else(|| header::HeaderValue::from_static("application/octet-stream"));
    resp.headers_mut().insert(header::CONTENT_TYPE, ct);
    resp.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("private, max-age=31536000, immutable"),
    );
    Ok(resp.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_hex_validation() {
        assert!(is_sha256_hex(&"a".repeat(64)));
        assert!(is_sha256_hex(&hex::encode(Sha256::digest(b"hi"))));
        assert!(!is_sha256_hex(&"A".repeat(64)), "uppercase rejected");
        assert!(!is_sha256_hex(&"a".repeat(63)), "wrong length rejected");
        assert!(!is_sha256_hex(&"g".repeat(64)), "non-hex rejected");
    }

    async fn test_pool(test_name: &str) -> Option<sqlx::PgPool> {
        let url = crate::routes::gateway::test_db_url(test_name)?;
        Some(
            sqlx::postgres::PgPoolOptions::new()
                .max_connections(2)
                .connect(&url)
                .await
                .expect("connect test db"),
        )
    }

    #[tokio::test]
    async fn put_stores_idempotently_and_get_serves() {
        let Some(pool) = test_pool("put_stores_idempotently_and_get_serves").await else {
            return;
        };
        let bytes = b"cct739-blob-payload".to_vec();
        let hash = hex::encode(Sha256::digest(&bytes));
        sqlx::query("DELETE FROM daemon_blobs WHERE hash = $1")
            .bind(&hash)
            .execute(&pool)
            .await
            .unwrap();

        let insert = || {
            let pool = pool.clone();
            let hash = hash.clone();
            let bytes = bytes.clone();
            async move {
                sqlx::query(
                    "INSERT INTO daemon_blobs (hash, media_type, byte_len, bytes) \
                     VALUES ($1, $2, $3, $4) ON CONFLICT (hash) DO NOTHING",
                )
                .bind(&hash)
                .bind(Some("image/png"))
                .bind(i64::try_from(bytes.len()).unwrap())
                .bind(bytes.as_slice())
                .execute(&pool)
                .await
                .unwrap()
                .rows_affected()
            }
        };
        assert_eq!(insert().await, 1, "first insert stores");
        assert_eq!(insert().await, 0, "re-insert is idempotent");

        let (mt, got): (Option<String>, Vec<u8>) =
            sqlx::query_as("SELECT media_type, bytes FROM daemon_blobs WHERE hash = $1")
                .bind(&hash)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(mt.as_deref(), Some("image/png"));
        assert_eq!(got, bytes);

        sqlx::query("DELETE FROM daemon_blobs WHERE hash = $1")
            .bind(&hash)
            .execute(&pool)
            .await
            .unwrap();
    }
}
