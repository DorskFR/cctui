//! Working-directory autocomplete + git facts for the spawn dialog.
//!
//! `GET /machines/{machine_id}/fs/dirs?path=…` asks the machine's daemon for
//! the sub-directories of `path` and returns their names.
//! `GET /machines/{machine_id}/fs/gitinfo?path=…` returns the branch / detached
//! HEAD of `path` (the daemon refuses paths outside its allowed roots).
//! `GET /machines/{machine_id}/fs/file?path=…&session_id=…` serves one file
//! an agent linked in a message: small files stream back inline (sniffed
//! content type, `ETag` by content hash), large ones redirect to the session's
//! content-addressed blob the daemon uploaded.
//! The daemon answers over its existing WS with the same `request_id` + oneshot pattern as
//! mid-chat file staging. Ownership rule matches spawn: the machine
//! must belong to the requesting user (admin tokens may browse any machine) —
//! no path restriction beyond that, since machine owners can already spawn
//! arbitrary commands.

use std::fmt::Write as _;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Extension, Json};
use base64::Engine;
use cctui_proto::git::GitInfo;
use cctui_proto::media::{is_inline_type, sniff_media_type};
use cctui_proto::models::{Liveness, SessionStatus};
use cctui_proto::ws::{READ_FILE_MAX_BYTES, ReadFileErrorKind, ReadFileOk};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::AuthContext;
use crate::bus;
use crate::error::AppError;
use crate::state::AppState;

type SessionLivenessRow = (Option<Uuid>, Option<String>, String, DateTime<Utc>, DateTime<Utc>);

#[derive(Debug, Deserialize)]
pub struct ListDirsParams {
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct ListDirsResponse {
    pub dirs: Vec<String>,
}

// Machine ownership is enforced by the `Resource(Machine, Read, IdFrom::Path
// ("machine_id"))` guard in `authz.rs`: the `authz_layer` middleware
// resolves `machines.user_id` and applies `admin || owner == caller` before this
// handler runs (404 unknown machine / 403 not-your-machine / admin bypass). The
// handler only needs the machine id to talk to the daemon.
pub async fn list_dirs(
    State(state): State<AppState>,
    Path(machine_id): Path<String>,
    Query(params): Query<ListDirsParams>,
) -> Result<Json<ListDirsResponse>, AppError> {
    let machine_uuid = Uuid::parse_str(&machine_id)
        .map_err(|_| AppError::new(StatusCode::BAD_REQUEST, "machine_id must be a uuid"))?;

    match bus::list_dirs(&state, machine_uuid, params.path).await {
        Ok(dirs) => Ok(Json(ListDirsResponse { dirs })),
        Err(bus::BusError::NoDaemon(_)) => {
            Err(AppError::new(StatusCode::SERVICE_UNAVAILABLE, "daemon offline"))
        }
        Err(bus::BusError::Timeout) => {
            Err(AppError::new(StatusCode::GATEWAY_TIMEOUT, "timed out waiting for the daemon"))
        }
        Err(bus::BusError::ListDirs(msg)) => Err(AppError::new(StatusCode::BAD_REQUEST, msg)),
        Err(e) => Err(AppError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    }
}

#[derive(Debug, Deserialize)]
pub struct GitInfoParams {
    pub path: String,
    /// Opt into the `git status` dirty check (subprocess on the daemon).
    #[serde(default)]
    pub dirty: bool,
}

/// Same ownership guard as [`list_dirs`].
pub async fn git_info(
    State(state): State<AppState>,
    Path(machine_id): Path<String>,
    Query(params): Query<GitInfoParams>,
) -> Result<Json<GitInfo>, AppError> {
    let machine_uuid = Uuid::parse_str(&machine_id)
        .map_err(|_| AppError::new(StatusCode::BAD_REQUEST, "machine_id must be a uuid"))?;

    match bus::git_info(&state, machine_uuid, params.path, params.dirty).await {
        Ok(info) => Ok(Json(info)),
        Err(bus::BusError::NoDaemon(_)) => {
            Err(AppError::new(StatusCode::SERVICE_UNAVAILABLE, "daemon offline"))
        }
        Err(bus::BusError::Timeout) => {
            Err(AppError::new(StatusCode::GATEWAY_TIMEOUT, "timed out waiting for the daemon"))
        }
        Err(bus::BusError::GitInfo(msg)) => Err(AppError::new(StatusCode::BAD_REQUEST, msg)),
        Err(e) => Err(AppError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    }
}

#[derive(Debug, Deserialize)]
pub struct ReadFileParams {
    pub path: String,
    /// Session the link came from. Required: it is what the path grant, the
    /// liveness gate and the session-read authz are all resolved against.
    pub session_id: String,
}

/// Machine ownership is the outer guard; this handler adds the three checks
/// that make the route conversation-scoped rather than a `$HOME` read:
/// session-read authz for the caller, a live (non-archived, non-dead) session,
/// and a path the agent actually linked in that session.
pub async fn read_file(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(machine_id): Path<String>,
    Query(params): Query<ReadFileParams>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let machine_uuid = Uuid::parse_str(&machine_id)
        .map_err(|_| AppError::new(StatusCode::BAD_REQUEST, "machine_id must be a uuid"))?;
    let path = params.path.trim().to_owned();
    if path.is_empty() {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "path is required"));
    }
    let sid = params.session_id.trim().to_owned();
    if sid.is_empty() {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "session_id is required"));
    }

    let cwd = authorize_read(&state.pool, &ctx, machine_uuid, &sid, &path).await?;

    let file =
        match bus::read_file(&state, machine_uuid, path.clone(), READ_FILE_MAX_BYTES, cwd).await {
            Ok(file) => file,
            Err(bus::BusError::NoDaemon(_)) => {
                return Err(AppError::new(StatusCode::SERVICE_UNAVAILABLE, "daemon offline"));
            }
            Err(bus::BusError::Timeout) => {
                return Err(AppError::new(
                    StatusCode::GATEWAY_TIMEOUT,
                    "timed out waiting for the daemon",
                ));
            }
            Err(bus::BusError::ReadFile(kind, msg)) => {
                let status = read_error_status(kind);
                tracing::warn!(%machine_id, %path, ?kind, %msg, "read-file refused");
                return Err(AppError::new(status, msg));
            }
            Err(e) => return Err(AppError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
        };

    if let Some(hash) = file.blob_hash.as_deref() {
        record_blob_link(&state.pool, &sid, hash).await?;
    }
    file_response(&file, Some(sid.as_str()), &headers)
}

/// The three gates that make `fs/file` conversation-scoped, in the order that
/// leaks least: caller may read the session, the session is live, the path was
/// linked in it. Returns the session's working dir for the daemon's own root
/// check.
async fn authorize_read(
    pool: &sqlx::PgPool,
    ctx: &AuthContext,
    machine_uuid: Uuid,
    sid: &str,
    path: &str,
) -> Result<Option<String>, AppError> {
    crate::authz::authorize_session_read(ctx, sid, pool)
        .await
        .map_err(|status| AppError::new(status, "not allowed to read this session"))?;

    let row: Option<SessionLivenessRow> = sqlx::query_as(
        "SELECT machine_uuid, working_dir, status, registered_at, last_heartbeat \
             FROM sessions WHERE id = $1",
    )
    .bind(sid)
    .fetch_optional(pool)
    .await?;
    let Some((machine, cwd, status, registered_at, last_heartbeat)) = row else {
        return Err(AppError::new(StatusCode::NOT_FOUND, "unknown session_id"));
    };
    if machine != Some(machine_uuid) {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "session_id does not belong to this machine",
        ));
    }

    let (session_status, liveness) =
        crate::routes::sessions::resolve_status_liveness(&status, registered_at, last_heartbeat);
    if session_status == SessionStatus::Archived || liveness == Liveness::Dead {
        tracing::warn!(%sid, %path, ?session_status, ?liveness, "read-file refused: session not live");
        return Err(AppError::new(StatusCode::FORBIDDEN, "session is no longer live"));
    }

    if !path_is_linked(pool, sid, path).await? {
        tracing::warn!(%sid, %path, "read-file refused: path not linked in this session");
        return Err(AppError::new(StatusCode::FORBIDDEN, "path was not linked in this session"));
    }
    Ok(cwd)
}

const MAX_LINKS_PER_EVENT: usize = 64;

async fn path_is_linked(
    pool: &sqlx::PgPool,
    session_id: &str,
    path: &str,
) -> Result<bool, sqlx::Error> {
    let hit: Option<i32> =
        sqlx::query_scalar("SELECT 1 FROM session_file_links WHERE session_id = $1 AND path = $2")
            .bind(session_id)
            .bind(path)
            .fetch_optional(pool)
            .await?;
    Ok(hit.is_some())
}

async fn record_blob_link(
    pool: &sqlx::PgPool,
    session_id: &str,
    hash: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO session_blob_links (session_id, hash) VALUES ($1, $2) \
         ON CONFLICT DO NOTHING",
    )
    .bind(session_id)
    .bind(hash)
    .execute(pool)
    .await?;
    Ok(())
}

/// Grant every local path and blob reference an event mentions, so the paths
/// the renderer will linkify are exactly the ones the read route will serve.
pub async fn record_links(
    pool: &sqlx::PgPool,
    session_id: &str,
    (paths, blobs): &(Vec<String>, Vec<String>),
) -> Result<(), sqlx::Error> {
    if !paths.is_empty() {
        sqlx::query(
            "INSERT INTO session_file_links (session_id, path) \
             SELECT $1, * FROM UNNEST($2::text[]) ON CONFLICT DO NOTHING",
        )
        .bind(session_id)
        .bind(paths)
        .execute(pool)
        .await?;
    }
    if !blobs.is_empty() {
        sqlx::query(
            "INSERT INTO session_blob_links (session_id, hash) \
             SELECT $1, * FROM UNNEST($2::text[]) ON CONFLICT DO NOTHING",
        )
        .bind(session_id)
        .bind(blobs)
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// Rust twin of `markdown.ts`'s `LOCAL_PATH`: absolute or `~/`-relative paths
/// with a file extension, plus the `blob_id` of every embedded blob reference.
#[must_use]
pub fn extract_links(payload: &serde_json::Value) -> (Vec<String>, Vec<String>) {
    let mut paths = std::collections::BTreeSet::new();
    let mut blobs = std::collections::BTreeSet::new();
    walk(payload, &mut paths, &mut blobs);
    (
        paths.into_iter().take(MAX_LINKS_PER_EVENT).collect(),
        blobs.into_iter().take(MAX_LINKS_PER_EVENT).collect(),
    )
}

fn walk(
    v: &serde_json::Value,
    paths: &mut std::collections::BTreeSet<String>,
    blobs: &mut std::collections::BTreeSet<String>,
) {
    match v {
        serde_json::Value::String(s) => scan_paths(s, paths),
        serde_json::Value::Array(a) => a.iter().for_each(|x| walk(x, paths, blobs)),
        serde_json::Value::Object(o) => {
            for (k, val) in o {
                if k == "blob_id"
                    && let Some(h) = val.as_str()
                    && h.len() == 64
                    && h.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                {
                    blobs.insert(h.to_owned());
                    continue;
                }
                walk(val, paths, blobs);
            }
        }
        _ => {}
    }
}

const fn is_path_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'@' | b'+' | b'%' | b'-' | b'/')
}

/// A candidate starts at a word boundary, so a URL's path (`https://h/a.png`)
/// never matches: its `//` is preceded by `:` and `h/a.png` does not start
/// with `/`.
fn scan_paths(s: &str, out: &mut std::collections::BTreeSet<String>) {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let lead_ok = i == 0
            || matches!(bytes[i - 1], b' ' | b'\t' | b'\n' | b'\r' | b'(' | b'[' | b';' | b'>')
            || matches!(bytes[i - 1], b'"' | b'\'' | b'`' | b',' | b'=' | b'*');
        let starts = bytes[i] == b'/' || (bytes[i] == b'~' && bytes.get(i + 1) == Some(&b'/'));
        if !(lead_ok && starts) {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < bytes.len() && is_path_byte(bytes[j]) {
            j += 1;
        }
        let mut cand = &s[i..j];
        while cand.ends_with('.') {
            cand = &cand[..cand.len() - 1];
        }
        if has_extension(cand) {
            out.insert(cand.to_owned());
        }
        i = j.max(i + 1);
    }
}

fn has_extension(cand: &str) -> bool {
    let name = &cand[cand.rfind('/').map_or(0, |p| p + 1)..];
    name.rfind('.').is_some_and(|dot| {
        let ext = &name[dot + 1..];
        !ext.is_empty()
            && ext.len() <= 8
            && ext.bytes().all(|b| b.is_ascii_alphanumeric())
            && dot > 0
    })
}

const fn read_error_status(kind: ReadFileErrorKind) -> StatusCode {
    match kind {
        ReadFileErrorKind::Denied => StatusCode::FORBIDDEN,
        ReadFileErrorKind::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        ReadFileErrorKind::NotFound => StatusCode::NOT_FOUND,
        ReadFileErrorKind::Io => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// `Content-Disposition` value: `inline` for viewable types, `attachment`
/// otherwise; the filename is quoted with `"` / `\` escaped and non-ASCII
/// carried in the RFC 5987 `filename*` form.
fn content_disposition(media_type: &str, name: &str) -> String {
    let kind = if is_inline_type(media_type) { "inline" } else { "attachment" };
    let ascii: String =
        name.chars()
            .map(|c| {
                if c.is_ascii() && !c.is_ascii_control() && c != '"' && c != '\\' { c } else { '_' }
            })
            .collect();
    let mut value = format!("{kind}; filename=\"{ascii}\"");
    if !name.is_ascii() {
        let _ = write!(value, "; filename*=UTF-8''{}", percent_encode(name));
    }
    value
}

/// RFC 3986 unreserved characters pass, everything else is `%XX`.
fn percent_encode(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

fn file_response(
    file: &ReadFileOk,
    session_id: Option<&str>,
    headers: &HeaderMap,
) -> Result<Response, AppError> {
    let etag = format!("\"{}\"", file.sha256);
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|t| t.trim() == etag))
    {
        let mut resp = StatusCode::NOT_MODIFIED.into_response();
        resp.headers_mut().insert(header::ETAG, etag.parse().expect("hex etag"));
        return Ok(resp);
    }

    if let Some(hash) = file.blob_hash.as_deref() {
        let Some(sid) = session_id else {
            return Err(AppError::new(
                StatusCode::CONFLICT,
                "session_id is required to fetch a file this large",
            ));
        };
        let target =
            format!("/api/v1/sessions/{}/blobs/{}", percent_encode(sid), percent_encode(hash));
        return Ok(Redirect::temporary(&target).into_response());
    }

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(file.data.as_deref().unwrap_or_default())
        .map_err(|_| {
            AppError::new(StatusCode::BAD_GATEWAY, "daemon returned an undecodable payload")
        })?;
    let media_type = sniff_media_type(&file.name, &bytes);
    let disposition = content_disposition(media_type, &file.name);

    let mut resp = Response::new(axum::body::Body::from(bytes));
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, header::HeaderValue::from_static(media_type));
    h.insert(header::CONTENT_DISPOSITION, disposition.parse().expect("ascii disposition"));
    h.insert(header::ETAG, etag.parse().expect("hex etag"));
    h.insert(header::X_CONTENT_TYPE_OPTIONS, header::HeaderValue::from_static("nosniff"));
    h.insert(header::CACHE_CONTROL, header::HeaderValue::from_static("private, no-cache"));
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(name: &str, bytes: &[u8]) -> ReadFileOk {
        ReadFileOk {
            name: name.into(),
            size: bytes.len() as u64,
            sha256: "deadbeef".into(),
            media_type: None,
            data: Some(base64::engine::general_purpose::STANDARD.encode(bytes)),
            blob_hash: None,
        }
    }

    fn header(resp: &Response, name: header::HeaderName) -> &str {
        resp.headers().get(name).and_then(|v| v.to_str().ok()).unwrap_or("")
    }

    #[test]
    fn inline_text_is_sniffed_and_served_inline_with_etag() {
        let resp = file_response(&ok("report.md", b"# hi\n"), None, &HeaderMap::new()).unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(header(&resp, header::CONTENT_TYPE), "text/markdown; charset=utf-8");
        assert_eq!(header(&resp, header::CONTENT_DISPOSITION), "inline; filename=\"report.md\"");
        assert_eq!(header(&resp, header::ETAG), "\"deadbeef\"");
        assert_eq!(header(&resp, header::X_CONTENT_TYPE_OPTIONS), "nosniff");
    }

    #[test]
    fn html_is_never_active_and_binaries_download() {
        let resp =
            file_response(&ok("x.html", b"<script>1</script>"), None, &HeaderMap::new()).unwrap();
        assert_eq!(header(&resp, header::CONTENT_TYPE), "text/plain; charset=utf-8");
        let resp = file_response(&ok("x.bin", &[0, 1, 2]), None, &HeaderMap::new()).unwrap();
        assert_eq!(header(&resp, header::CONTENT_TYPE), "application/octet-stream");
        assert_eq!(header(&resp, header::CONTENT_DISPOSITION), "attachment; filename=\"x.bin\"");
    }

    #[test]
    fn filename_is_escaped_and_non_ascii_gets_rfc5987_form() {
        assert_eq!(
            content_disposition("text/plain", "a\"b\\c.txt"),
            "inline; filename=\"a_b_c.txt\""
        );
        assert_eq!(
            content_disposition("application/octet-stream", "é.bin"),
            "attachment; filename=\"_.bin\"; filename*=UTF-8''%C3%A9.bin"
        );
    }

    #[test]
    fn matching_if_none_match_short_circuits_to_304() {
        let mut headers = HeaderMap::new();
        headers.insert(header::IF_NONE_MATCH, "\"deadbeef\"".parse().unwrap());
        let resp = file_response(&ok("a.txt", b"x"), None, &headers).unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_MODIFIED);
    }

    #[test]
    fn large_files_redirect_to_the_session_blob_and_need_a_session() {
        let mut file = ok("big.zip", b"");
        file.data = None;
        file.blob_hash = Some("ab".repeat(32));
        let resp = file_response(&file, Some("sess 1"), &HeaderMap::new()).unwrap();
        assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(
            header(&resp, header::LOCATION),
            format!("/api/v1/sessions/sess%201/blobs/{}", "ab".repeat(32))
        );
        let err = file_response(&file, None, &HeaderMap::new()).unwrap_err();
        assert!(matches!(err, AppError::Status(StatusCode::CONFLICT, _)));
    }

    #[test]
    fn only_linked_local_paths_are_extracted() {
        let payload = serde_json::json!({
            "content": "wrote /home/u/out/report.md and ~/shots/x.png, see https://ex.com/a/b.png",
            "nested": [{ "text": "also `/tmp/c.log`." }],
            "source": { "type": "cctui-blob", "blob_id": "ab".repeat(32) },
        });
        let (paths, blobs) = extract_links(&payload);
        assert_eq!(paths, vec!["/home/u/out/report.md", "/tmp/c.log", "~/shots/x.png"]);
        assert_eq!(blobs, vec!["ab".repeat(32)]);
    }

    #[test]
    fn bare_urls_directories_and_extensionless_paths_are_not_grants() {
        let payload = serde_json::json!({
            "a": "https://ok.example/dir/file.md",
            "b": "cd /home/u/proj and /usr/bin",
            "c": "a/b.txt and 1/2.5",
            "d": "http://h/etc/passwd.conf",
        });
        assert_eq!(extract_links(&payload).0, Vec::<String>::new());
    }

    struct Fixture {
        pool: sqlx::PgPool,
        machine: Uuid,
        owner: AuthContext,
        stranger: AuthContext,
        session: String,
    }

    fn ctx(user_id: Uuid) -> AuthContext {
        AuthContext {
            user_id,
            key_id: Uuid::new_v4(),
            machine_id: None,
            scopes: std::collections::BTreeSet::new(),
        }
    }

    async fn fixture(tag: &str) -> Option<Fixture> {
        let url = crate::routes::gateway::test_db_url(tag)?;
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let (owner, stranger, machine) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let session = format!("cct985-{tag}");
        for (uid, name) in [(owner, "owner"), (stranger, "stranger")] {
            sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
                .bind(uid)
                .bind(format!("{tag}-{name}"))
                .bind(format!("{tag}-{name}-hash"))
                .execute(&pool)
                .await
                .unwrap();
        }
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, $3, $4)")
            .bind(machine)
            .bind(owner)
            .bind(tag)
            .bind(format!("{tag}-mkey"))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status) \
             VALUES ($1, $2, $2, $3, '/home/u/proj', 'active')",
        )
        .bind(&session)
        .bind(machine)
        .bind(owner)
        .execute(&pool)
        .await
        .unwrap();
        Some(Fixture { pool, machine, owner: ctx(owner), stranger: ctx(stranger), session })
    }

    async fn cleanup(f: &Fixture) {
        sqlx::query("DELETE FROM sessions WHERE id = $1")
            .bind(&f.session)
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM machines WHERE id = $1")
            .bind(f.machine)
            .execute(&f.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM users WHERE id = ANY($1)")
            .bind(vec![f.owner.user_id, f.stranger.user_id])
            .execute(&f.pool)
            .await
            .unwrap();
    }

    fn status_of(e: &AppError) -> StatusCode {
        match e {
            AppError::Status(s, _) => *s,
            other => panic!("expected a status error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_ungranted_path_is_refused_and_a_granted_one_passes_every_gate() {
        let Some(f) = fixture("ungranted_path_is_refused").await else { return };
        let secret = "/home/u/.ssh/id_ed25519";
        let linked = "/home/u/proj/out/report.md";

        let err =
            authorize_read(&f.pool, &f.owner, f.machine, &f.session, secret).await.unwrap_err();
        assert_eq!(status_of(&err), StatusCode::FORBIDDEN, "ungranted path");
        let err =
            authorize_read(&f.pool, &f.owner, f.machine, &f.session, linked).await.unwrap_err();
        assert_eq!(status_of(&err), StatusCode::FORBIDDEN, "under cwd but never linked");

        let payload = serde_json::json!({ "content": format!("wrote {linked} for you") });
        record_links(&f.pool, &f.session, &extract_links(&payload)).await.unwrap();

        assert_eq!(
            authorize_read(&f.pool, &f.owner, f.machine, &f.session, linked).await.unwrap(),
            Some("/home/u/proj".to_owned())
        );
        let err =
            authorize_read(&f.pool, &f.owner, f.machine, &f.session, secret).await.unwrap_err();
        assert_eq!(status_of(&err), StatusCode::FORBIDDEN, "grants do not widen to siblings");
        cleanup(&f).await;
    }

    #[tokio::test]
    async fn another_users_session_is_refused_even_with_a_grant() {
        let Some(f) = fixture("another_users_session_is_refused").await else { return };
        let linked = "/home/u/proj/out/report.md";
        let payload = serde_json::json!({ "content": linked });
        record_links(&f.pool, &f.session, &extract_links(&payload)).await.unwrap();

        assert!(authorize_read(&f.pool, &f.owner, f.machine, &f.session, linked).await.is_ok());
        let err =
            authorize_read(&f.pool, &f.stranger, f.machine, &f.session, linked).await.unwrap_err();
        assert_eq!(status_of(&err), StatusCode::FORBIDDEN);

        let err = authorize_read(&f.pool, &f.owner, f.machine, "no-such-session", linked)
            .await
            .unwrap_err();
        assert_eq!(status_of(&err), StatusCode::NOT_FOUND, "unknown sessions never leak as 403");
        cleanup(&f).await;
    }

    #[tokio::test]
    async fn archived_ended_and_dead_sessions_are_refused() {
        let Some(f) = fixture("dead_sessions_are_refused").await else { return };
        let linked = "/home/u/proj/out/report.md";
        record_links(&f.pool, &f.session, &extract_links(&serde_json::json!(linked)))
            .await
            .unwrap();

        let set = |status: &'static str, heartbeat_age_secs: i64| {
            let pool = f.pool.clone();
            let sid = f.session.clone();
            async move {
                sqlx::query(
                    "UPDATE sessions SET status = $2, \
                     last_heartbeat = now() - make_interval(secs => $3) WHERE id = $1",
                )
                .bind(&sid)
                .bind(status)
                .bind(heartbeat_age_secs as f64)
                .execute(&pool)
                .await
                .unwrap();
            }
        };

        set("active", 60).await;
        assert!(
            authorize_read(&f.pool, &f.owner, f.machine, &f.session, linked).await.is_ok(),
            "a live session serves"
        );
        set("active", 15 * 60).await;
        assert!(
            authorize_read(&f.pool, &f.owner, f.machine, &f.session, linked).await.is_ok(),
            "stale (under an hour) still serves"
        );

        for (status, age) in [("active", 2 * 3600), ("archived", 60), ("ended", 60), ("failed", 60)]
        {
            set(status, age).await;
            let err =
                authorize_read(&f.pool, &f.owner, f.machine, &f.session, linked).await.unwrap_err();
            assert_eq!(status_of(&err), StatusCode::FORBIDDEN, "{status} @ {age}s must refuse");
        }
        cleanup(&f).await;
    }

    #[tokio::test]
    async fn a_grant_does_not_cross_sessions_or_machines() {
        let Some(f) = fixture("grant_does_not_cross_sessions").await else { return };
        let linked = "/home/u/proj/out/report.md";
        record_links(&f.pool, &f.session, &extract_links(&serde_json::json!(linked)))
            .await
            .unwrap();

        let other_machine = Uuid::new_v4();
        let err =
            authorize_read(&f.pool, &f.owner, other_machine, &f.session, linked).await.unwrap_err();
        assert_eq!(status_of(&err), StatusCode::BAD_REQUEST, "session must live on the machine");

        let other = format!("{}-b", f.session);
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status) \
             VALUES ($1, $2, $2, $3, '/home/u/proj', 'active')",
        )
        .bind(&other)
        .bind(f.machine)
        .bind(f.owner.user_id)
        .execute(&f.pool)
        .await
        .unwrap();
        let err = authorize_read(&f.pool, &f.owner, f.machine, &other, linked).await.unwrap_err();
        assert_eq!(status_of(&err), StatusCode::FORBIDDEN, "grants are per-session");
        sqlx::query("DELETE FROM sessions WHERE id = $1")
            .bind(&other)
            .execute(&f.pool)
            .await
            .unwrap();
        cleanup(&f).await;
    }

    #[test]
    fn refusal_kinds_map_to_statuses() {
        assert_eq!(read_error_status(ReadFileErrorKind::Denied), StatusCode::FORBIDDEN);
        assert_eq!(read_error_status(ReadFileErrorKind::TooLarge), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(read_error_status(ReadFileErrorKind::NotFound), StatusCode::NOT_FOUND);
        assert_eq!(read_error_status(ReadFileErrorKind::Io), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
