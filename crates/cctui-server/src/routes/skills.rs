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
use crate::skill_store::{SkillError, SkillStore, validate_name};
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
         ON CONFLICT (uploaded_by_user, name) DO UPDATE SET \
            version = EXCLUDED.version, sha256 = EXCLUDED.sha256, \
            size_bytes = EXCLUDED.size_bytes, \
            uploaded_by_machine = EXCLUDED.uploaded_by_machine, \
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
    let upload = Upload {
        name: &name,
        version: &version,
        claimed_hash: claimed_hash.as_deref(),
        content_type: &content_type,
        machine_id,
        user_id,
    };
    let row = store_upload(&state.pool, &state.skills, upload, reader).await?;

    tracing::info!(
        name = %name,
        machine_id = %machine_id,
        version = %version,
        sha256 = %row.2,
        size_bytes = row.3,
        "skill upload"
    );

    Ok(Json(row_to_entry(row)))
}

struct Upload<'a> {
    name: &'a str,
    version: &'a str,
    claimed_hash: Option<&'a str>,
    content_type: &'a str,
    machine_id: Uuid,
    user_id: Uuid,
}

/// Write the bundle under the uploader's own directory and upsert the
/// uploader's own row; another account's skill of the same name is untouched.
async fn store_upload<R: tokio::io::AsyncRead + Unpin>(
    pool: &sqlx::PgPool,
    store: &SkillStore,
    u: Upload<'_>,
    reader: R,
) -> Result<Row, StatusCode> {
    let stats = store.write(u.user_id, u.name, reader).await.map_err(|e| match e {
        SkillError::InvalidName => StatusCode::BAD_REQUEST,
        SkillError::Io(err) => {
            tracing::error!("skill write io error: {err}");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    })?;

    if let Some(claimed) = u.claimed_hash
        && claimed != stats.sha256
    {
        let _ = tokio::fs::remove_file(store.path_of(u.user_id, u.name)).await;
        return Err(StatusCode::CONFLICT);
    }

    let size_i64 = i64::try_from(stats.size_bytes).unwrap_or(i64::MAX);
    upsert_entry(
        pool,
        NewEntry {
            name: u.name,
            version: u.version,
            sha256: &stats.sha256,
            size_bytes: size_i64,
            machine_id: u.machine_id,
            user_id: u.user_id,
            content_type: u.content_type,
        },
    )
    .await
    .map_err(|e| {
        tracing::error!("skill registry upsert error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })
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

    let (bundle, file) = open_bundle(&state.pool, &state.skills, user_id, &name).await?;
    let Bundle { sha256, size_bytes, content_type } = bundle;
    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);
    let mut resp = Response::new(body);
    resp.headers_mut().insert(header::CONTENT_TYPE, content_type.parse().unwrap());
    resp.headers_mut().insert(header::CONTENT_LENGTH, size_bytes.to_string().parse().unwrap());
    resp.headers_mut().insert("X-CCTUI-SHA256", sha256.parse().unwrap());
    Ok(resp.into_response())
}

struct Bundle {
    sha256: String,
    size_bytes: i64,
    content_type: String,
}

/// `user_id`'s own row and bundle for `name`. A bundle still in the ownerless
/// layout is adopted on first read when it matches the row's hash.
async fn open_bundle(
    pool: &sqlx::PgPool,
    store: &SkillStore,
    user_id: Uuid,
    name: &str,
) -> Result<(Bundle, tokio::fs::File), StatusCode> {
    let row: Option<(String, i64, String)> = sqlx::query_as(
        "SELECT sha256, size_bytes, content_type FROM skill_registry \
         WHERE name = $1 AND uploaded_by_user = $2",
    )
    .bind(name)
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| {
        tracing::error!("skill get db error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let (sha256, size_bytes, content_type) = row.ok_or(StatusCode::NOT_FOUND)?;

    let path = store.path_of(user_id, name);
    let file = match tokio::fs::File::open(&path).await {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if let Err(err) = store.adopt_legacy(user_id, name, &sha256).await {
                tracing::error!(%name, "legacy skill adopt error: {err}");
            }
            tokio::fs::File::open(&path).await
        }
        other => other,
    }
    .map_err(|e| {
        tracing::error!(path = %path.display(), "skill get open error: {e}");
        StatusCode::NOT_FOUND
    })?;
    Ok((Bundle { sha256, size_bytes, content_type }, file))
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

    async fn seed_user(pool: &sqlx::PgPool, tag: &str) -> (Uuid, Uuid) {
        let suffix = Uuid::new_v4();
        let user_id: Uuid = sqlx::query_scalar(
            "INSERT INTO users (id, name, key_hash) \
             VALUES (gen_random_uuid(), $1, gen_random_uuid()::text) RETURNING id",
        )
        .bind(format!("{tag}-{suffix}"))
        .fetch_one(pool)
        .await
        .unwrap();
        let machine_id: Uuid = sqlx::query_scalar(
            "INSERT INTO machines (id, user_id, name, key_hash) \
             VALUES (gen_random_uuid(), $1, $2, gen_random_uuid()::text) RETURNING id",
        )
        .bind(user_id)
        .bind(format!("host-{suffix}"))
        .fetch_one(pool)
        .await
        .unwrap();
        (user_id, machine_id)
    }

    async fn read_all(mut f: tokio::fs::File) -> Vec<u8> {
        use tokio::io::AsyncReadExt;
        let mut out = Vec::new();
        f.read_to_end(&mut out).await.unwrap();
        out
    }

    #[tokio::test]
    async fn two_users_upload_the_same_name_and_each_reads_their_own() {
        let Some(url) = test_db_url("two_users_upload_the_same_skill_name") else {
            return;
        };
        let pool = sqlx::PgPool::connect(&url).await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let store = SkillStore::new(dir.path().to_path_buf());
        let (alice, alice_machine) = seed_user(&pool, "skills-a").await;
        let (bob, bob_machine) = seed_user(&pool, "skills-b").await;
        let name = format!("shared-{}", Uuid::new_v4().simple());
        let upload = |user_id, machine_id, version| Upload {
            name: &name,
            version,
            claimed_hash: None,
            content_type: DEFAULT_CONTENT_TYPE,
            machine_id,
            user_id,
        };

        let a_row = store_upload(&pool, &store, upload(alice, alice_machine, "a1"), &b"alice"[..])
            .await
            .unwrap();
        let (a_before, _) = open_bundle(&pool, &store, alice, &name).await.unwrap();
        store_upload(&pool, &store, upload(bob, bob_machine, "b1"), &b"bob-bytes"[..])
            .await
            .unwrap();

        let (a, a_file) = open_bundle(&pool, &store, alice, &name).await.unwrap();
        assert_eq!(read_all(a_file).await, b"alice");
        assert_eq!(a.sha256, a_before.sha256, "bob's upload must not touch alice's row");
        assert_eq!(a.size_bytes, 5);
        let (b, b_file) = open_bundle(&pool, &store, bob, &name).await.unwrap();
        assert_eq!(read_all(b_file).await, b"bob-bytes");
        assert_ne!(a.sha256, b.sha256);

        let alice_entries = list_entries(&pool, alice).await.unwrap();
        assert_eq!(alice_entries.len(), 1);
        assert_eq!(
            alice_entries[0], a_row,
            "alice's row is unchanged, machine and version included"
        );

        sqlx::query("DELETE FROM users WHERE id = ANY($1)")
            .bind(vec![alice, bob])
            .execute(&pool)
            .await
            .unwrap();
    }
}
