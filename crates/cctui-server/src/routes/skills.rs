use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use cctui_proto::api::SkillIndexEntry;
use futures_util::TryStreamExt;
use tokio_util::io::{ReaderStream, StreamReader};
use uuid::Uuid;

use crate::auth::{AuthContext, TokenRole};
use crate::skill_store::{SkillError, validate_name};
use crate::state::AppState;

const DEFAULT_CONTENT_TYPE: &str = "application/zstd";

const fn require_machine(ctx: &AuthContext) -> Result<(Uuid, Uuid), StatusCode> {
    match (ctx.role, ctx.machine_id, ctx.user_id) {
        (TokenRole::Machine, Some(mid), Some(uid)) => Ok((mid, uid)),
        _ => Err(StatusCode::FORBIDDEN),
    }
}

const fn require_user_scope(ctx: &AuthContext) -> Result<Uuid, StatusCode> {
    match (ctx.role, ctx.user_id) {
        (TokenRole::Machine | TokenRole::User, Some(uid)) => Ok(uid),
        _ => Err(StatusCode::FORBIDDEN),
    }
}

type Row = (String, String, String, i64, Option<Uuid>, chrono::DateTime<chrono::Utc>, String);

fn row_to_entry(r: Row) -> SkillIndexEntry {
    SkillIndexEntry {
        name: r.0,
        version: r.1,
        sha256: r.2,
        size_bytes: r.3,
        uploaded_by_machine: r.4,
        uploaded_at: r.5,
        content_type: r.6,
    }
}

#[allow(clippy::similar_names)]
pub async fn put(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(name): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Result<Json<SkillIndexEntry>, StatusCode> {
    let (machine_id, user_id) = require_machine(&ctx)?;
    if validate_name(&name).is_err() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let claimed_hash =
        headers.get("X-CCTUI-SHA256").and_then(|v| v.to_str().ok()).map(str::to_ascii_lowercase);
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty())
        .map_or_else(|| DEFAULT_CONTENT_TYPE.to_string(), str::to_string);

    let stream = body.into_data_stream().map_err(std::io::Error::other);
    let reader = StreamReader::new(stream);

    let stats = state.skills.write(&name, reader).await.map_err(|e| match e {
        SkillError::InvalidName => StatusCode::BAD_REQUEST,
        SkillError::Io(err) => {
            tracing::error!("skill write io error: {err}");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    })?;

    if let Some(claimed) = claimed_hash
        && claimed != stats.sha256
    {
        let _ = tokio::fs::remove_file(state.skills.path_of(&name)).await;
        return Err(StatusCode::CONFLICT);
    }

    let size_i64 = i64::try_from(stats.size_bytes).unwrap_or(i64::MAX);
    let row: Row = sqlx::query_as(
        "INSERT INTO skill_registry \
         (name, version, sha256, size_bytes, uploaded_by_machine, uploaded_by_user, content_type) \
         VALUES ($1,$2,$3,$4,$5,$6,$7) \
         ON CONFLICT (name) DO UPDATE SET \
            version = EXCLUDED.version, sha256 = EXCLUDED.sha256, \
            size_bytes = EXCLUDED.size_bytes, \
            uploaded_by_machine = EXCLUDED.uploaded_by_machine, \
            uploaded_by_user = EXCLUDED.uploaded_by_user, \
            uploaded_at = now(), content_type = EXCLUDED.content_type \
         RETURNING name, version, sha256, size_bytes, uploaded_by_machine, uploaded_at, \
                   content_type",
    )
    .bind(&name)
    .bind(&stats.sha256)
    .bind(&stats.sha256)
    .bind(size_i64)
    .bind(machine_id)
    .bind(user_id)
    .bind(&content_type)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("skill registry upsert error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    tracing::info!(
        name = %name,
        machine_id = %machine_id,
        sha256 = %stats.sha256,
        size_bytes = stats.size_bytes,
        "skill upload"
    );

    Ok(Json(row_to_entry(row)))
}

pub async fn index(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<Vec<SkillIndexEntry>>, StatusCode> {
    let user_id = require_user_scope(&ctx)?;
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT name, version, sha256, size_bytes, uploaded_by_machine, uploaded_at, \
                content_type \
         FROM skill_registry \
         WHERE uploaded_by_user = $1 \
         ORDER BY name ASC",
    )
    .bind(user_id)
    .fetch_all(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("skill index db error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(rows.into_iter().map(row_to_entry).collect()))
}

pub async fn get(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(name): Path<String>,
) -> Result<Response, StatusCode> {
    let user_id = require_user_scope(&ctx)?;
    if validate_name(&name).is_err() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let row: Option<(String, i64, String)> = sqlx::query_as(
        "SELECT sha256, size_bytes, content_type FROM skill_registry \
         WHERE name = $1 AND uploaded_by_user = $2",
    )
    .bind(&name)
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("skill get db error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let (sha256, size_bytes, content_type) = row.ok_or(StatusCode::NOT_FOUND)?;

    let path = state.skills.path_of(&name);
    let file = tokio::fs::File::open(&path).await.map_err(|e| {
        tracing::error!(path = %path.display(), "skill get open error: {e}");
        StatusCode::NOT_FOUND
    })?;
    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);
    let mut resp = Response::new(body);
    resp.headers_mut().insert(header::CONTENT_TYPE, content_type.parse().unwrap());
    resp.headers_mut().insert(header::CONTENT_LENGTH, size_bytes.to_string().parse().unwrap());
    resp.headers_mut().insert("X-CCTUI-SHA256", sha256.parse().unwrap());
    Ok(resp.into_response())
}
