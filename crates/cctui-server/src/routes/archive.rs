use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::{Extension, Json};
use futures_util::TryStreamExt;
use serde::{Deserialize, Serialize};
use tokio_util::io::StreamReader;
use uuid::Uuid;

use crate::archive_store::ArchiveError;
use crate::auth::{AuthContext, TokenRole};
use crate::state::AppState;

#[derive(Deserialize)]
pub struct HeadQuery {
    pub sha256: String,
}

#[derive(Serialize)]
pub struct PutResponse {
    pub sha256: String,
    pub size_bytes: u64,
    pub line_count: u32,
}

const fn require_machine(ctx: &AuthContext) -> Result<Uuid, StatusCode> {
    match (ctx.role, ctx.machine_id) {
        (TokenRole::Machine, Some(mid)) => Ok(mid),
        _ => Err(StatusCode::FORBIDDEN),
    }
}

fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && !s.contains('/')
        && !s.contains('\\')
        && !s.contains('\0')
}

pub async fn head(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path((project_dir, session_id)): Path<(String, String)>,
    Query(q): Query<HeadQuery>,
) -> Result<StatusCode, StatusCode> {
    let machine_id = require_machine(&ctx)?;
    if !valid_name(&project_dir) || !valid_name(&session_id) || q.sha256.len() != 64 {
        return Err(StatusCode::BAD_REQUEST);
    }

    let row: Option<(String,)> = sqlx::query_as(
        "SELECT sha256 FROM archive_index WHERE machine_id = $1 AND session_id = $2",
    )
    .bind(machine_id)
    .bind(&session_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("archive head db error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    match row {
        Some((hash,)) if hash.eq_ignore_ascii_case(&q.sha256) => Ok(StatusCode::NO_CONTENT),
        _ => Err(StatusCode::NOT_FOUND),
    }
}

#[allow(clippy::similar_names)]
pub async fn put(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path((project_dir, session_id)): Path<(String, String)>,
    headers: HeaderMap,
    body: Body,
) -> Result<Json<PutResponse>, StatusCode> {
    let machine_id = require_machine(&ctx)?;
    if !valid_name(&project_dir) || !valid_name(&session_id) {
        return Err(StatusCode::BAD_REQUEST);
    }

    let claimed_hash =
        headers.get("X-CCTUI-SHA256").and_then(|v| v.to_str().ok()).map(str::to_ascii_lowercase);

    let stream = body.into_data_stream().map_err(std::io::Error::other);
    let reader = StreamReader::new(stream);

    let stats =
        state.archive.write(machine_id, &project_dir, &session_id, reader).await.map_err(|e| {
            match e {
                ArchiveError::InvalidName => StatusCode::BAD_REQUEST,
                ArchiveError::Io(err) => {
                    tracing::error!("archive write io error: {err}");
                    StatusCode::INTERNAL_SERVER_ERROR
                }
            }
        })?;

    if let Some(claimed) = claimed_hash
        && claimed != stats.sha256
    {
        let _ =
            tokio::fs::remove_file(state.archive.path_of(machine_id, &project_dir, &session_id))
                .await;
        return Err(StatusCode::CONFLICT);
    }

    sqlx::query(
        "INSERT INTO archive_index \
         (machine_id, project_dir, session_id, sha256, size_bytes, line_count) \
         VALUES ($1,$2,$3,$4,$5,$6) \
         ON CONFLICT (machine_id, session_id) DO UPDATE SET \
             sha256 = EXCLUDED.sha256, size_bytes = EXCLUDED.size_bytes, \
             line_count = EXCLUDED.line_count, uploaded_at = now(), \
             project_dir = EXCLUDED.project_dir",
    )
    .bind(machine_id)
    .bind(&project_dir)
    .bind(&session_id)
    .bind(&stats.sha256)
    .bind(i64::try_from(stats.size_bytes).unwrap_or(i64::MAX))
    .bind(i32::try_from(stats.line_count).unwrap_or(i32::MAX))
    .execute(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("archive index upsert error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    tracing::info!(
        machine_id = %machine_id,
        project_dir = %project_dir,
        session_id = %session_id,
        size_bytes = stats.size_bytes,
        "archive upload"
    );

    Ok(Json(PutResponse {
        sha256: stats.sha256,
        size_bytes: stats.size_bytes,
        line_count: stats.line_count,
    }))
}
