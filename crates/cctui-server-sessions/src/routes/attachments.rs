//! Files the user sent (`paste-N.txt` masks, screenshots, docs), whether in the
//! spawn modal or mid-chat.
//!
//! `POST /sessions/spawn` and `POST /sessions/{id}/files` both, through
//! [`record_uploads`], keep a copy in the content-addressed blob store with a
//! `session_attachments` row per file, before the bytes are staged on the
//! daemon. A draft keeps its files the same way under the draft id
//! (`PUT /sessions/{id}/draft-attachments` replaces them) until its launch
//! re-reads them into the spawn. `GET /api/v1/sessions/{id}/attachments`
//! lists those rows (session-read authz via the `api_router` layer); the bytes
//! come from the existing `GET /sessions/{id}/blobs/{hash}`.

use crate::ports::SessionsBlobs;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use cctui_proto::media::sniff_media_type;

use crate::context::SessionsContext;
use crate::uploads::RawUpload;

pub use cctui_proto::api::attachments::SessionAttachment;

fn media_type_for(upload: &RawUpload) -> String {
    upload
        .content_type
        .as_deref()
        .map(|ct| ct.split(';').next().unwrap_or(ct).trim())
        .filter(|ct| !ct.is_empty() && *ct != "application/octet-stream")
        .map_or_else(|| sniff_media_type(&upload.name, &upload.bytes).to_owned(), str::to_owned)
}

/// Store each upload as a blob and record it against `session_id`.
///
/// `staged_names` are the daemon's final (collision-suffixed) names, index
/// aligned with `uploads`; a missing entry falls back to the upload name.
pub async fn record_uploads<B: SessionsBlobs>(
    pool: &sqlx::PgPool,
    session_id: &str,
    uploads: &[RawUpload],
    staged_names: &[String],
) -> Result<Vec<SessionAttachment>, sqlx::Error> {
    let mut conn = pool.acquire().await?;
    record_uploads_in::<B>(&mut conn, session_id, uploads, staged_names).await
}

/// [`record_uploads`] on an explicit connection, so a caller can pair it with
/// other writes in one transaction.
pub async fn record_uploads_in<B: SessionsBlobs>(
    conn: &mut sqlx::PgConnection,
    session_id: &str,
    uploads: &[RawUpload],
    staged_names: &[String],
) -> Result<Vec<SessionAttachment>, sqlx::Error> {
    let mut out = Vec::with_capacity(uploads.len());
    for (i, upload) in uploads.iter().enumerate() {
        let media_type = media_type_for(upload);
        let stored = B::store_blob_in(&mut *conn, &upload.bytes, Some(&media_type)).await?;
        let name = staged_names.get(i).map_or(upload.name.as_str(), String::as_str);
        let row: SessionAttachment = sqlx::query_as(
            "INSERT INTO session_attachments (session_id, name, hash, size, content_type) \
             VALUES ($1, $2, $3, $4, $5) \
             RETURNING id, session_id, message_id, name, hash, size, content_type, \
                       (extract(epoch FROM created_at) * 1000)::bigint AS created_at_ms, \
                       (SELECT s.machine_id FROM sessions s WHERE s.id = $1) AS machine_id",
        )
        .bind(session_id)
        .bind(name)
        .bind(&stored.hash)
        .bind(i64::try_from(upload.bytes.len()).unwrap_or(i64::MAX))
        .bind(&media_type)
        .fetch_one(&mut *conn)
        .await?;
        out.push(row);
    }
    Ok(out)
}

/// Undo a [`record_uploads`] batch whose operation then failed. Blobs stay:
/// they are content-addressed and may be shared with another attachment.
pub async fn delete_attachments(
    pool: &sqlx::PgPool,
    ids: &[uuid::Uuid],
) -> Result<(), sqlx::Error> {
    if ids.is_empty() {
        return Ok(());
    }
    sqlx::query("DELETE FROM session_attachments WHERE id = ANY($1)")
        .bind(ids)
        .execute(pool)
        .await?;
    Ok(())
}

/// Adopt the daemon's final (collision-suffixed) staged names.
///
/// The conversation
/// matches chips to the paths printed under the message, so the row has to carry
/// the staged name — which is only known once staging has succeeded.
pub async fn set_attachment_names(
    pool: &sqlx::PgPool,
    rows: &[SessionAttachment],
    staged_names: &[String],
) -> Result<(), sqlx::Error> {
    for (row, staged) in rows.iter().zip(staged_names) {
        if row.name == *staged {
            continue;
        }
        sqlx::query("UPDATE session_attachments SET name = $2 WHERE id = $1")
            .bind(row.id)
            .bind(staged)
            .execute(pool)
            .await?;
    }
    Ok(())
}

pub async fn list_session_attachments(
    pool: &sqlx::PgPool,
    session_id: &str,
) -> Result<Vec<SessionAttachment>, sqlx::Error> {
    sqlx::query_as(
        "SELECT a.id, a.session_id, a.message_id, a.name, a.hash, a.size, a.content_type, \
                (extract(epoch FROM a.created_at) * 1000)::bigint AS created_at_ms, \
                s.machine_id \
         FROM session_attachments a LEFT JOIN sessions s ON s.id = a.session_id \
         WHERE a.session_id = $1 ORDER BY a.created_at, a.id",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await
}

pub async fn get_session_attachments<S: SessionsContext>(
    State(state): State<S>,
    Path(session_id): Path<String>,
) -> Result<Json<Vec<SessionAttachment>>, StatusCode> {
    list_session_attachments(state.pool(), &session_id).await.map(Json).map_err(|e| {
        tracing::error!(%session_id, "attachments list: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::Bytes;

    fn upload(name: &str, bytes: &[u8], content_type: Option<&str>) -> RawUpload {
        RawUpload {
            name: name.to_owned(),
            bytes: Bytes::copy_from_slice(bytes),
            content_type: content_type.map(str::to_owned),
        }
    }

    #[test]
    fn media_type_prefers_the_part_header_then_sniffs() {
        assert_eq!(
            media_type_for(&upload("a.txt", b"hi", Some("text/plain; charset=utf-8"))),
            "text/plain"
        );
        let png = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0];
        assert_eq!(media_type_for(&upload("shot.png", &png, None)), "image/png");
        assert_eq!(
            media_type_for(&upload("shot.png", &png, Some("application/octet-stream"))),
            "image/png"
        );
    }
}
