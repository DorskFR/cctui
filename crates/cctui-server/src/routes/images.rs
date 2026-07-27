//! Agent-posted image blob store.
//!
//! `POST /api/v1/daemon/sessions/{id}/images` — the daemon uploads an image it
//! detected as an `![alt](/abs/path.png)` marker in an assistant message. Raw
//! bytes body, machine-key Bearer (self-authenticating like the other
//! `daemon/sessions/{id}/…` endpoints), user-scoped to the machine's owner. The
//! media type is sniffed from magic bytes (the `Content-Type` header is not
//! trusted) and must be in the png/jpeg/gif/webp allow-list. Dedups by sha256
//! within a session; enforces a size cap + per-session quota.
//!
//! `GET /api/v1/sessions/{id}/images/{image_id}` — the webui fetches the blob to
//! render inline. Session-read authz (enforced by the `api_router` authz layer)
//! + the same-origin `HttpOnly` cookie, so a plain `<img src>` works.

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use cctui_proto::api::{ApiError, SessionImageUploadResponse};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::state::AppState;

/// Per-image byte cap, matching the inbound attachment budget (`uploads.rs`).
pub const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
/// Per-session quotas so a runaway agent can't fill the blob store.
pub const MAX_IMAGES_PER_SESSION: i64 = 200;
pub const MAX_BYTES_PER_SESSION: i64 = 100 * 1024 * 1024;

type ApiErr = (StatusCode, axum::Json<ApiError>);

fn err(code: StatusCode, msg: impl Into<String>) -> ApiErr {
    (code, axum::Json(ApiError { error: msg.into() }))
}

/// Sniff an image media type from magic bytes. Returns `None` for anything
/// outside the png/jpeg/gif/webp allow-list, so a non-image (or a mislabeled
/// upload) is rejected regardless of the `Content-Type` header.
#[must_use]
pub fn sniff_image_media_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some("image/png");
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

async fn machine_user(state: &AppState, headers: &header::HeaderMap) -> Result<Uuid, ApiErr> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| err(StatusCode::UNAUTHORIZED, "missing bearer token"))?;
    let ctx = state
        .auth_config
        .validate(token)
        .await
        .ok_or_else(|| err(StatusCode::UNAUTHORIZED, "invalid token"))?;
    if ctx.machine_id.is_none() {
        return Err(err(StatusCode::FORBIDDEN, "machine token required"));
    }
    Ok(ctx.user_id)
}

pub async fn upload_session_image(
    State(state): State<AppState>,
    headers: header::HeaderMap,
    Path(session_id): Path<String>,
    body: Bytes,
) -> Result<axum::Json<SessionImageUploadResponse>, ApiErr> {
    let user_id = machine_user(&state, &headers).await?;

    if body.len() > MAX_IMAGE_BYTES {
        return Err(err(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("image is {} bytes; per-image cap is {MAX_IMAGE_BYTES}", body.len()),
        ));
    }
    let Some(media_type) = sniff_image_media_type(&body) else {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "unsupported image type (png/jpeg/gif/webp only)",
        ));
    };

    // User-scope: only accept images for a session owned by the machine's user.
    // A missing row 404s; a foreign row 403s (never leak another user's store).
    let owner: Option<Uuid> = sqlx::query_scalar("SELECT user_id FROM sessions WHERE id = $1")
        .bind(&session_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(|e| {
            tracing::error!("image upload owner lookup: {e}");
            err(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        })?;
    match owner {
        None => return Err(err(StatusCode::NOT_FOUND, "session not found")),
        Some(o) if o != user_id => return Err(err(StatusCode::FORBIDDEN, "not your session")),
        Some(_) => {}
    }

    let sha256 = hex::encode(Sha256::digest(&body));

    // Dedup: re-posting the same bytes (e.g. a reconcile replay) returns the
    // existing id without a second copy.
    let existing: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM session_images WHERE session_id = $1 AND sha256 = $2")
            .bind(&session_id)
            .bind(&sha256)
            .fetch_optional(&state.pool)
            .await
            .map_err(|e| {
                tracing::error!("image dedup lookup: {e}");
                err(StatusCode::INTERNAL_SERVER_ERROR, "database error")
            })?;
    if let Some(id) = existing {
        return Ok(axum::Json(SessionImageUploadResponse { image_id: id.to_string() }));
    }

    let (count, total): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COALESCE(SUM(byte_len), 0) FROM session_images WHERE session_id = $1",
    )
    .bind(&session_id)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("image quota lookup: {e}");
        err(StatusCode::INTERNAL_SERVER_ERROR, "database error")
    })?;
    let byte_len = i64::try_from(body.len()).unwrap_or(i64::MAX);
    if count >= MAX_IMAGES_PER_SESSION || total + byte_len > MAX_BYTES_PER_SESSION {
        return Err(err(StatusCode::PAYLOAD_TOO_LARGE, "session image quota exceeded"));
    }

    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO session_images (session_id, sha256, media_type, byte_len, bytes) \
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(&session_id)
    .bind(&sha256)
    .bind(media_type)
    .bind(byte_len)
    .bind(body.as_ref())
    .fetch_one(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("image insert: {e}");
        err(StatusCode::INTERNAL_SERVER_ERROR, "database error")
    })?;

    Ok(axum::Json(SessionImageUploadResponse { image_id: id.to_string() }))
}

pub async fn get_session_image(
    State(state): State<AppState>,
    Path((session_id, image_id)): Path<(String, Uuid)>,
) -> Result<Response, StatusCode> {
    // Session-read authz is enforced upstream by the `api_router` authz layer
    // (`sess_read`, id from the `{id}` path param); the image id must also belong
    // to this session so one session's ref can't fetch another's blob.
    let row: Option<(String, Vec<u8>)> = sqlx::query_as(
        "SELECT media_type, bytes FROM session_images WHERE id = $1 AND session_id = $2",
    )
    .bind(image_id)
    .bind(&session_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("image get: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let (media_type, bytes) = row.ok_or(StatusCode::NOT_FOUND)?;

    let mut resp = Response::new(axum::body::Body::from(bytes));
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        media_type.parse().unwrap_or_else(|_| header::HeaderValue::from_static("image/png")),
    );
    // Immutable: a blob id maps to fixed bytes forever.
    resp.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("private, max-age=31536000, immutable"),
    );
    Ok(resp.into_response())
}

#[cfg(test)]
mod tests {
    use super::sniff_image_media_type;

    #[test]
    fn sniffs_the_allowed_types() {
        assert_eq!(
            sniff_image_media_type(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0]),
            Some("image/png")
        );
        assert_eq!(sniff_image_media_type(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        assert_eq!(sniff_image_media_type(b"GIF89a...."), Some("image/gif"));
        let mut webp = b"RIFF".to_vec();
        webp.extend_from_slice(&[0, 0, 0, 0]);
        webp.extend_from_slice(b"WEBPmore");
        assert_eq!(sniff_image_media_type(&webp), Some("image/webp"));
    }

    #[test]
    fn rejects_non_images() {
        assert_eq!(sniff_image_media_type(b"not an image"), None);
        assert_eq!(sniff_image_media_type(b""), None);
        // A RIFF container that isn't WEBP (e.g. WAV) is rejected.
        let mut wav = b"RIFF".to_vec();
        wav.extend_from_slice(&[0, 0, 0, 0]);
        wav.extend_from_slice(b"WAVEfmt ");
        assert_eq!(sniff_image_media_type(&wav), None);
    }
}
