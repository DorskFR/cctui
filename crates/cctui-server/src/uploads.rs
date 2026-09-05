//! Shared multipart→`BootstrapFile` parsing for file uploads.
//!
//! Both `POST /sessions/spawn` (spawn-time bootstrap uploads) and
//! `POST /sessions/{id}/files` (mid-chat attachments) accept the same
//! `multipart/form-data` shape and enforce the same caps. Keeping the parsing,
//! base64 encoding, and cap validation here means there is a **single encoding
//! point**: a future move off base64 (binary frames / presigned upload) changes
//! this one helper + the `BootstrapFile` proto struct and fixes both routes at
//! once.

use axum::Json;
use axum::body::Bytes;
use axum::extract::Multipart;
use axum::http::StatusCode;
use base64::Engine;
use cctui_proto::adapter::BootstrapFile;
use cctui_proto::api::ApiError;

/// Upload caps. The bytes ride the server→daemon WS leg as base64 inside a
/// single JSON frame, so this is deliberately an "attach a screenshot / small
/// doc" budget, not bulk transfer. A route's `DefaultBodyLimit` should be set
/// above [`MAX_TOTAL_BYTES`] so an over-cap upload is rejected here with a clear
/// 413 rather than a generic body-limit error.
pub const MAX_FILE_BYTES: usize = 5 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: usize = 20 * 1024 * 1024;
pub const MAX_FILES: usize = 10;

type ApiErr = (StatusCode, Json<ApiError>);

fn bad_request(msg: impl Into<String>) -> ApiErr {
    (StatusCode::BAD_REQUEST, Json(ApiError { error: msg.into() }))
}

fn too_large(msg: impl Into<String>) -> ApiErr {
    (StatusCode::PAYLOAD_TOO_LARGE, Json(ApiError { error: msg.into() }))
}

/// Reduce an uploaded filename to a safe bare name: strip any directory
/// component and reject traversal (`..`)/empty/`.`. Prevents a malicious `name`
/// from escaping the per-session staging dir.
pub fn sanitize_upload_name(raw: &str) -> Result<String, ApiErr> {
    let base = std::path::Path::new(raw)
        .file_name()
        .and_then(|s| s.to_str())
        .map(str::to_owned)
        .ok_or_else(|| bad_request(format!("invalid upload filename: {raw:?}")))?;
    if base.is_empty() || base == ".." || base == "." {
        return Err(bad_request(format!("invalid upload filename: {raw:?}")));
    }
    Ok(base)
}

/// Result of parsing an upload multipart body: the file parts (base64-encoded,
/// caps enforced) plus, when present, the raw text of a `request` part for
/// routes that carry a JSON sidecar (spawn).
pub struct ParsedUploads {
    pub files: Vec<BootstrapFile>,
    pub request_json: Option<String>,
    /// Same order as `files`, still raw: what the blob store copies.
    pub raw: Vec<RawUpload>,
}

pub struct RawUpload {
    pub name: String,
    pub bytes: Bytes,
    pub content_type: Option<String>,
}

/// Drain a `multipart/form-data` body into [`BootstrapFile`]s, enforcing the
/// per-file / total / count caps. Any part with a `filename` is a file; a part
/// named `request` is captured as `request_json`; other non-file parts are
/// ignored.
pub async fn parse_upload_multipart(mut multipart: Multipart) -> Result<ParsedUploads, ApiErr> {
    let mut files: Vec<BootstrapFile> = Vec::new();
    let mut request_json: Option<String> = None;
    let mut raw: Vec<RawUpload> = Vec::new();
    let mut total_bytes = 0usize;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| bad_request(format!("malformed multipart body: {e}")))?
    {
        let field_name = field.name().map(str::to_owned);
        let file_name = field.file_name().map(str::to_owned);
        if let Some(raw_name) = file_name {
            let name = sanitize_upload_name(&raw_name)?;
            let content_type = field.content_type().map(str::to_owned);
            let bytes = field
                .bytes()
                .await
                .map_err(|e| bad_request(format!("reading upload {name:?}: {e}")))?;
            if bytes.len() > MAX_FILE_BYTES {
                return Err(too_large(format!(
                    "file {name:?} is {} bytes; per-file cap is {MAX_FILE_BYTES}",
                    bytes.len()
                )));
            }
            total_bytes += bytes.len();
            if total_bytes > MAX_TOTAL_BYTES {
                return Err(too_large(format!(
                    "uploads exceed the {MAX_TOTAL_BYTES}-byte total cap"
                )));
            }
            files.push(BootstrapFile {
                name: name.clone(),
                content_b64: base64::engine::general_purpose::STANDARD.encode(&bytes),
            });
            raw.push(RawUpload { name, bytes, content_type });
            if files.len() > MAX_FILES {
                return Err(too_large(format!("too many files; cap is {MAX_FILES}")));
            }
        } else if field_name.as_deref() == Some("request") {
            let raw = field
                .text()
                .await
                .map_err(|e| bad_request(format!("reading request part: {e}")))?;
            request_json = Some(raw);
        }
        // Unknown non-file parts are ignored.
    }

    Ok(ParsedUploads { files, request_json, raw })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err_status(raw: &str) -> StatusCode {
        sanitize_upload_name(raw).unwrap_err().0
    }

    #[test]
    fn plain_names_pass_through() {
        assert_eq!(sanitize_upload_name("evil.txt").unwrap(), "evil.txt");
        assert_eq!(sanitize_upload_name("a-b_c.1.png").unwrap(), "a-b_c.1.png");
    }

    #[test]
    fn directory_components_are_stripped_to_the_bare_name() {
        assert_eq!(sanitize_upload_name("../../../etc/passwd").unwrap(), "passwd");
        assert_eq!(sanitize_upload_name("/etc/shadow").unwrap(), "shadow");
        assert_eq!(sanitize_upload_name("a/b/c.png").unwrap(), "c.png");
        assert_eq!(sanitize_upload_name("./sub/./x").unwrap(), "x");
    }

    #[test]
    fn traversal_dot_and_empty_are_rejected() {
        assert_eq!(err_status(".."), StatusCode::BAD_REQUEST);
        assert_eq!(err_status("."), StatusCode::BAD_REQUEST);
        assert_eq!(err_status(""), StatusCode::BAD_REQUEST);
        assert_eq!(err_status("foo/.."), StatusCode::BAD_REQUEST);
        assert_eq!(err_status("a/b/../.."), StatusCode::BAD_REQUEST);
        assert_eq!(err_status("/"), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn result_never_contains_a_path_separator() {
        for raw in ["../../secret", "/abs/path/file", "nested/dir/name.bin", "plain.txt"] {
            if let Ok(name) = sanitize_upload_name(raw) {
                assert!(
                    !name.contains('/'),
                    "{raw:?} sanitized to a name with a separator: {name:?}"
                );
                assert_ne!(name, "..");
                assert_ne!(name, ".");
                assert!(!name.is_empty());
            }
        }
    }
}
