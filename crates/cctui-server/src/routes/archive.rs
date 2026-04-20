use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use cctui_proto::api::{
    ArchiveIndexEntry, ArchiveStatusEntry, ArchiveStatusResponse, ArchiveSyncState,
    ManifestPostRequest,
};
use cctui_proto::ws::ServerEvent;
use futures_util::TryStreamExt;
use serde::{Deserialize, Serialize};
use tokio_util::io::{ReaderStream, StreamReader};
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

/// Read access: a Machine token can read any archive belonging to the same
/// user (so `cctui-admin pull` from host B can fetch archives uploaded by
/// host A). Users/admins are not accepted — pulls happen on enrolled hosts.
const fn require_user_scope(ctx: &AuthContext) -> Result<Uuid, StatusCode> {
    match (ctx.role, ctx.user_id) {
        (TokenRole::Machine | TokenRole::User, Some(uid)) => Ok(uid),
        _ => Err(StatusCode::FORBIDDEN),
    }
}

type IndexRow = (Uuid, String, String, String, i64, Option<i32>, chrono::DateTime<chrono::Utc>);

#[derive(Deserialize)]
pub struct GetQuery {
    /// Disambiguate when the same `session_id` exists on multiple machines of
    /// the same user. Optional: if omitted and only one match exists under
    /// this `user_id`, that one is served.
    pub machine_id: Option<Uuid>,
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

    // Broadcast so open web-UI clients can flip the pill to `synced` live (CCT-68).
    let _ = state.tui_tx.send(ServerEvent::ArchiveUploaded {
        machine_id,
        project_dir: project_dir.clone(),
        session_id: session_id.clone(),
        size_bytes: i64::try_from(stats.size_bytes).unwrap_or(i64::MAX),
        sha256: stats.sha256.clone(),
    });

    Ok(Json(PutResponse {
        sha256: stats.sha256,
        size_bytes: stats.size_bytes,
        line_count: stats.line_count,
    }))
}

/// Replace this machine's manifest of expected archive files (CCT-68).
///
/// Transactional: delete all prior rows for this machine and insert the new set
/// atomically so a consumer never sees a half-applied manifest.
pub async fn post_manifest(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(body): Json<ManifestPostRequest>,
) -> Result<StatusCode, StatusCode> {
    let machine_id = require_machine(&ctx)?;

    // Validate every entry before touching the DB.
    for e in &body.entries {
        if !valid_name(&e.project_dir) || !valid_name(&e.session_id) {
            return Err(StatusCode::BAD_REQUEST);
        }
    }

    let mut tx = state.pool.begin().await.map_err(|e| {
        tracing::error!("manifest begin tx: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    sqlx::query("DELETE FROM archive_manifest WHERE machine_id = $1")
        .bind(machine_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| {
            tracing::error!("manifest delete: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    for e in &body.entries {
        sqlx::query(
            "INSERT INTO archive_manifest \
             (machine_id, project_dir, session_id, size_bytes, mtime) \
             VALUES ($1,$2,$3,$4,$5)",
        )
        .bind(machine_id)
        .bind(&e.project_dir)
        .bind(&e.session_id)
        .bind(e.size_bytes)
        .bind(e.mtime)
        .execute(&mut *tx)
        .await
        .map_err(|err| {
            tracing::error!("manifest insert: {err}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    }

    tx.commit().await.map_err(|e| {
        tracing::error!("manifest commit: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let count = i64::try_from(body.entries.len()).unwrap_or(i64::MAX);
    tracing::info!(machine_id = %machine_id, count, "archive manifest posted");
    let _ = state.tui_tx.send(ServerEvent::ArchiveManifest { machine_id, count });

    Ok(StatusCode::NO_CONTENT)
}

type StatusRow = (
    uuid::Uuid,
    String,
    String,
    i64,
    chrono::DateTime<chrono::Utc>,
    Option<i64>,
    Option<String>,
    Option<chrono::DateTime<chrono::Utc>>,
);

/// Compute per-(machine, session) sync status by left-joining `archive_manifest`
/// (expected) with `archive_index` (uploaded).
pub async fn get_status(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<ArchiveStatusResponse>, StatusCode> {
    let is_admin = ctx.role == TokenRole::Admin;
    let user_id = if is_admin { None } else { Some(require_user_scope(&ctx)?) };

    let base = "SELECT m.machine_id, m.project_dir, m.session_id, \
                m.size_bytes, m.mtime, \
                a.size_bytes, a.sha256, a.uploaded_at \
         FROM archive_manifest m \
         JOIN machines mx ON mx.id = m.machine_id \
         LEFT JOIN archive_index a \
                ON a.machine_id = m.machine_id AND a.session_id = m.session_id";
    let rows: Vec<StatusRow> = if let Some(uid) = user_id {
        sqlx::query_as(&format!(
            "{base} WHERE mx.user_id = $1 ORDER BY m.machine_id, m.project_dir, m.session_id"
        ))
        .bind(uid)
        .fetch_all(&state.pool)
        .await
    } else {
        sqlx::query_as(&format!("{base} ORDER BY m.machine_id, m.project_dir, m.session_id"))
            .fetch_all(&state.pool)
            .await
    }
    .map_err(|e| {
        tracing::error!("archive status db error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let entries = rows
        .into_iter()
        .map(
            |(
                machine_id,
                project_dir,
                session_id,
                expected_size,
                expected_mtime,
                uploaded_size,
                uploaded_sha256,
                uploaded_at,
            )| {
                let state = match uploaded_size {
                    None => ArchiveSyncState::Missing,
                    Some(us) if us < expected_size => ArchiveSyncState::Stale,
                    Some(_) => match uploaded_at {
                        Some(ua) if ua < expected_mtime => ArchiveSyncState::Stale,
                        _ => ArchiveSyncState::Synced,
                    },
                };
                ArchiveStatusEntry {
                    machine_id,
                    project_dir,
                    session_id,
                    expected_size,
                    expected_mtime,
                    uploaded_size,
                    uploaded_sha256,
                    uploaded_at,
                    state,
                }
            },
        )
        .collect();

    Ok(Json(ArchiveStatusResponse { entries }))
}

/// List all archives owned by the caller's user (across all their machines).
pub async fn index(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<Vec<ArchiveIndexEntry>>, StatusCode> {
    let is_admin = ctx.role == TokenRole::Admin;
    let user_id = if is_admin { None } else { Some(require_user_scope(&ctx)?) };
    let rows: Vec<IndexRow> = if let Some(uid) = user_id {
        sqlx::query_as(
            "SELECT a.machine_id, a.project_dir, a.session_id, a.sha256, a.size_bytes, \
                        a.line_count, a.uploaded_at \
                 FROM archive_index a \
                 JOIN machines m ON m.id = a.machine_id \
                 WHERE m.user_id = $1 \
                 ORDER BY a.uploaded_at DESC",
        )
        .bind(uid)
        .fetch_all(&state.pool)
        .await
    } else {
        sqlx::query_as(
            "SELECT a.machine_id, a.project_dir, a.session_id, a.sha256, a.size_bytes, \
                        a.line_count, a.uploaded_at \
                 FROM archive_index a \
                 ORDER BY a.uploaded_at DESC",
        )
        .fetch_all(&state.pool)
        .await
    }
    .map_err(|e| {
        tracing::error!("archive index db error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let entries = rows
        .into_iter()
        .map(
            |(machine_id, project_dir, session_id, sha256, size_bytes, line_count, uploaded_at)| {
                ArchiveIndexEntry {
                    machine_id,
                    project_dir,
                    session_id,
                    sha256,
                    size_bytes,
                    line_count,
                    uploaded_at,
                }
            },
        )
        .collect();
    Ok(Json(entries))
}

/// Stream the raw JSONL bytes for one archived session back to the caller.
pub async fn get(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path((project_dir, session_id)): Path<(String, String)>,
    Query(q): Query<GetQuery>,
) -> Result<Response, StatusCode> {
    let user_id = require_user_scope(&ctx)?;
    if !valid_name(&project_dir) || !valid_name(&session_id) {
        return Err(StatusCode::BAD_REQUEST);
    }

    // Resolve to a specific (machine_id, sha256, size) owned by this user.
    // When machine_id is omitted and more than one machine has this session
    // under this user, return 409 so the caller must disambiguate.
    let rows: Vec<(Uuid, String, i64)> = match q.machine_id {
        Some(mid) => {
            sqlx::query_as(
                "SELECT a.machine_id, a.sha256, a.size_bytes FROM archive_index a \
             JOIN machines m ON m.id = a.machine_id \
             WHERE m.user_id = $1 AND a.machine_id = $2 \
               AND a.project_dir = $3 AND a.session_id = $4",
            )
            .bind(user_id)
            .bind(mid)
            .bind(&project_dir)
            .bind(&session_id)
            .fetch_all(&state.pool)
            .await
        }
        None => {
            sqlx::query_as(
                "SELECT a.machine_id, a.sha256, a.size_bytes FROM archive_index a \
             JOIN machines m ON m.id = a.machine_id \
             WHERE m.user_id = $1 AND a.project_dir = $2 AND a.session_id = $3 \
             LIMIT 2",
            )
            .bind(user_id)
            .bind(&project_dir)
            .bind(&session_id)
            .fetch_all(&state.pool)
            .await
        }
    }
    .map_err(|e| {
        tracing::error!("archive get db error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let (machine_id, sha256, size_bytes) = match rows.len() {
        0 => return Err(StatusCode::NOT_FOUND),
        1 => rows.into_iter().next().unwrap(),
        _ => return Err(StatusCode::CONFLICT),
    };

    let path = state.archive.path_of(machine_id, &project_dir, &session_id);
    let file = tokio::fs::File::open(&path).await.map_err(|e| {
        tracing::error!(path = %path.display(), "archive get open error: {e}");
        StatusCode::NOT_FOUND
    })?;
    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);
    let mut resp = Response::new(body);
    resp.headers_mut().insert(header::CONTENT_TYPE, "application/x-ndjson".parse().unwrap());
    resp.headers_mut().insert(header::CONTENT_LENGTH, size_bytes.to_string().parse().unwrap());
    resp.headers_mut().insert("X-CCTUI-SHA256", sha256.parse().unwrap());
    resp.headers_mut().insert("X-CCTUI-Machine-Id", machine_id.to_string().parse().unwrap());
    Ok(resp.into_response())
}
