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

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use base64::Engine;
use cctui_proto::git::GitInfo;
use cctui_proto::media::{is_inline_type, sniff_media_type};
use cctui_proto::ws::{READ_FILE_MAX_BYTES, ReadFileErrorKind, ReadFileOk};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::bus;
use crate::error::AppError;
use crate::state::AppState;

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
    /// Session the link came from: widens the daemon's allow-list to its
    /// working directory and names the blob endpoint a large file redirects to.
    #[serde(default)]
    pub session_id: Option<String>,
}

/// Same ownership guard as [`list_dirs`]; the daemon enforces the path
/// allow-list and the size cap, and every refusal is logged here.
pub async fn read_file(
    State(state): State<AppState>,
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

    let mut cwd = None;
    if let Some(sid) = params.session_id.as_deref() {
        let row: Option<(Option<Uuid>, Option<String>)> =
            sqlx::query_as("SELECT machine_uuid, working_dir FROM sessions WHERE id = $1")
                .bind(sid)
                .fetch_optional(&state.pool)
                .await?;
        match row {
            Some((Some(m), wd)) if m == machine_uuid => cwd = wd,
            Some(_) => {
                return Err(AppError::new(
                    StatusCode::BAD_REQUEST,
                    "session_id does not belong to this machine",
                ));
            }
            None => return Err(AppError::new(StatusCode::NOT_FOUND, "unknown session_id")),
        }
    }

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

    file_response(&file, params.session_id.as_deref(), &headers)
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
    fn refusal_kinds_map_to_statuses() {
        assert_eq!(read_error_status(ReadFileErrorKind::Denied), StatusCode::FORBIDDEN);
        assert_eq!(read_error_status(ReadFileErrorKind::TooLarge), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(read_error_status(ReadFileErrorKind::NotFound), StatusCode::NOT_FOUND);
        assert_eq!(read_error_status(ReadFileErrorKind::Io), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
