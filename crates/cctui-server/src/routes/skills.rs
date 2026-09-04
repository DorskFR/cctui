use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use cctui_proto::api::SkillIndexEntry;
use futures_util::TryStreamExt;
use tokio_util::io::{ReaderStream, StreamReader};
use uuid::Uuid;

use crate::auth::AuthContext;
use crate::skill_store::{SkillError, validate_name};
use crate::state::AppState;

const DEFAULT_CONTENT_TYPE: &str = "application/zstd";
const VERSION_HEADER: &str = "X-CCTUI-Version";
const MAX_VERSION_LEN: usize = 128;

/// A skill upload is a machine-key action (the daemon publishes its skills).
fn require_machine(ctx: &AuthContext) -> Result<(Uuid, Uuid), StatusCode> {
    ctx.machine_id.map_or(Err(StatusCode::FORBIDDEN), |mid| Ok((mid, ctx.user_id)))
}

/// Reading the skill index/blobs needs any authenticated identity (machine or
/// human) with the `read` scope; it returns the owning user for scoping.
fn require_user_scope(ctx: &AuthContext) -> Result<Uuid, StatusCode> {
    ctx.requires(crate::auth::Scope::Read)?;
    Ok(ctx.user_id)
}

type Row = (String, String, String, i64, Option<Uuid>, chrono::DateTime<chrono::Utc>, String);

/// Version from `X-CCTUI-Version`, else the upload instant in unix
/// milliseconds so successive unlabelled uploads still sort in order.
/// A present-but-invalid header is a 400.
fn resolve_version(headers: &HeaderMap) -> Result<String, StatusCode> {
    let Some(raw) = headers.get(VERSION_HEADER) else {
        return Ok(chrono::Utc::now().timestamp_millis().to_string());
    };
    let v = raw.to_str().map_err(|_| StatusCode::BAD_REQUEST)?.trim();
    if v.is_empty() || v.len() > MAX_VERSION_LEN || v.chars().any(char::is_control) {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(v.to_string())
}

struct NewEntry<'a> {
    name: &'a str,
    version: &'a str,
    sha256: &'a str,
    size_bytes: i64,
    machine_id: Uuid,
    user_id: Uuid,
    content_type: &'a str,
}

async fn upsert_entry(pool: &sqlx::PgPool, e: NewEntry<'_>) -> sqlx::Result<Row> {
    sqlx::query_as(
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
    .bind(e.name)
    .bind(e.version)
    .bind(e.sha256)
    .bind(e.size_bytes)
    .bind(e.machine_id)
    .bind(e.user_id)
    .bind(e.content_type)
    .fetch_one(pool)
    .await
}

async fn list_entries(pool: &sqlx::PgPool, user_id: Uuid) -> sqlx::Result<Vec<Row>> {
    sqlx::query_as(
        "SELECT name, version, sha256, size_bytes, uploaded_by_machine, uploaded_at, \
                content_type \
         FROM skill_registry \
         WHERE uploaded_by_user = $1 \
         ORDER BY name ASC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
}

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

    let version = resolve_version(&headers)?;
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
    let row = upsert_entry(
        &state.pool,
        NewEntry {
            name: &name,
            version: &version,
            sha256: &stats.sha256,
            size_bytes: size_i64,
            machine_id,
            user_id,
            content_type: &content_type,
        },
    )
    .await
    .map_err(|e| {
        tracing::error!("skill registry upsert error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    tracing::info!(
        name = %name,
        machine_id = %machine_id,
        version = %version,
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
    let rows = list_entries(&state.pool, user_id).await.map_err(|e| {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::gateway::test_db_url;

    fn headers_with(v: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(VERSION_HEADER, v.parse().unwrap());
        h
    }

    #[test]
    fn version_header_wins() {
        assert_eq!(resolve_version(&headers_with(" 1.2.3 ")).unwrap(), "1.2.3");
    }

    #[test]
    fn version_defaults_to_unix_millis() {
        let before = chrono::Utc::now().timestamp_millis();
        let v: i64 = resolve_version(&HeaderMap::new()).unwrap().parse().unwrap();
        assert!(v >= before);
    }

    #[test]
    fn version_header_invalid_is_400() {
        assert_eq!(resolve_version(&headers_with("")).unwrap_err(), StatusCode::BAD_REQUEST);
        assert_eq!(
            resolve_version(&headers_with(&"x".repeat(MAX_VERSION_LEN + 1))).unwrap_err(),
            StatusCode::BAD_REQUEST
        );
    }

    #[tokio::test]
    async fn index_reports_version_distinct_from_sha256() {
        let Some(url) = test_db_url("index_reports_version_distinct_from_sha256") else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.unwrap();
        let suffix = Uuid::new_v4();
        let user_id: Uuid = sqlx::query_scalar(
            "INSERT INTO users (id, name, key_hash) \
             VALUES (gen_random_uuid(), $1, gen_random_uuid()::text) RETURNING id",
        )
        .bind(format!("skills-{suffix}"))
        .fetch_one(&pool)
        .await
        .unwrap();
        let machine_id: Uuid = sqlx::query_scalar(
            "INSERT INTO machines (id, user_id, name, key_hash) \
             VALUES (gen_random_uuid(), $1, $2, gen_random_uuid()::text) RETURNING id",
        )
        .bind(user_id)
        .bind(format!("host-{suffix}"))
        .fetch_one(&pool)
        .await
        .unwrap();
        let name = format!("skill-{}", suffix.simple());
        let sha = "a".repeat(64);

        upsert_entry(
            &pool,
            NewEntry {
                name: &name,
                version: "2026.09.04",
                sha256: &sha,
                size_bytes: 42,
                machine_id,
                user_id,
                content_type: DEFAULT_CONTENT_TYPE,
            },
        )
        .await
        .unwrap();

        let entries: Vec<SkillIndexEntry> =
            list_entries(&pool, user_id).await.unwrap().into_iter().map(row_to_entry).collect();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, name);
        assert_eq!(entries[0].version, "2026.09.04");
        assert_eq!(entries[0].sha256, sha);
        assert_ne!(entries[0].version, entries[0].sha256);
        assert_eq!(entries[0].uploaded_by_machine, Some(machine_id));

        sqlx::query("DELETE FROM users WHERE id = $1").bind(user_id).execute(&pool).await.unwrap();
    }
}
