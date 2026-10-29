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

pub use cctui_proto::api::uploads::UploadCaps;

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
pub async fn parse_upload_multipart(
    mut multipart: Multipart,
    caps: UploadCaps,
) -> Result<ParsedUploads, ApiErr> {
    let mut files: Vec<BootstrapFile> = Vec::new();
    let mut request_json: Option<String> = None;
    let mut raw: Vec<RawUpload> = Vec::new();
    let mut total_bytes = 0u64;

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
            if bytes.len() as u64 > caps.max_file_bytes {
                return Err(too_large(format!(
                    "file {name:?} is {} bytes; per-file cap is {}",
                    bytes.len(),
                    caps.max_file_bytes
                )));
            }
            total_bytes += bytes.len() as u64;
            if total_bytes > caps.max_total_bytes {
                return Err(too_large(format!(
                    "uploads exceed the {}-byte total cap",
                    caps.max_total_bytes
                )));
            }
            files.push(BootstrapFile {
                name: name.clone(),
                content_b64: base64::engine::general_purpose::STANDARD.encode(&bytes),
            });
            raw.push(RawUpload { name, bytes, content_type });
            if files.len() as u64 > u64::from(caps.max_files) {
                return Err(too_large(format!("too many files; cap is {}", caps.max_files)));
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

    const BOUNDARY: &str = "capsboundary";

    fn request_of(files: &[(&str, usize)]) -> axum::http::Request<axum::body::Body> {
        use std::fmt::Write as _;
        let mut body = String::new();
        for (name, size) in files {
            let _ = write!(
                body,
                "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"f\"; filename=\"{name}\"\r\n\r\n{}\r\n",
                "x".repeat(*size)
            );
        }
        let _ = write!(body, "--{BOUNDARY}--\r\n");
        axum::http::Request::builder()
            .method("POST")
            .uri("/u")
            .header("content-type", format!("multipart/form-data; boundary={BOUNDARY}"))
            .body(axum::body::Body::from(body))
            .expect("request")
    }

    /// `Ok(file count)` or `Err((status, message))`.
    ///
    /// Goes through a one-route service so the body limit is explicit: a bare
    /// `Multipart::from_request` would apply axum's 2 MB default and reject the
    /// default-cap bodies as malformed before any cap is checked. Neither
    /// `ParsedUploads` nor `ApiErr` is `Debug`, hence the plain return values.
    async fn parse(
        files: &[(&str, usize)],
        caps: UploadCaps,
    ) -> Result<usize, (StatusCode, String)> {
        use axum::response::IntoResponse;
        use tower::ServiceExt;

        let app = axum::Router::new()
            .route(
                "/u",
                axum::routing::post(move |mp: Multipart| async move {
                    match parse_upload_multipart(mp, caps).await {
                        Ok(p) => (StatusCode::OK, p.files.len().to_string()).into_response(),
                        Err((status, Json(e))) => (status, e.error).into_response(),
                    }
                }),
            )
            .layer(axum::extract::DefaultBodyLimit::disable());

        let res = app.oneshot(request_of(files)).await.expect("infallible");
        let status = res.status();
        let bytes =
            axum::body::to_bytes(res.into_body(), 64 * 1024 * 1024).await.expect("response body");
        let text = String::from_utf8_lossy(&bytes).to_string();
        if status == StatusCode::OK {
            Ok(text.parse().expect("file count"))
        } else {
            Err((status, text))
        }
    }

    #[test]
    fn built_in_caps_are_the_default() {
        let d = UploadCaps::default();
        assert_eq!(d.max_files, 10);
        assert_eq!(d.max_file_bytes, 5 * 1024 * 1024);
        assert_eq!(d.max_total_bytes, 20 * 1024 * 1024);
    }

    #[tokio::test]
    async fn uploads_within_the_configured_caps_pass() {
        let caps = UploadCaps { max_files: 2, max_file_bytes: 10, max_total_bytes: 20 };
        assert_eq!(parse(&[("a.txt", 10), ("b.txt", 10)], caps).await, Ok(2));
    }

    #[tokio::test]
    async fn a_file_over_the_configured_per_file_cap_is_413() {
        let caps = UploadCaps { max_files: 10, max_file_bytes: 8, max_total_bytes: 1024 };
        let (status, msg) = parse(&[("a.txt", 9)], caps).await.expect_err("over cap");
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
        assert!(msg.contains("per-file cap is 8"), "{msg}");
    }

    #[tokio::test]
    async fn more_files_than_the_configured_count_cap_is_413() {
        let caps = UploadCaps { max_files: 1, max_file_bytes: 1024, max_total_bytes: 1024 };
        let (status, msg) = parse(&[("a.txt", 1), ("b.txt", 1)], caps).await.expect_err("over cap");
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
        assert!(msg.contains("cap is 1"), "{msg}");
    }

    #[tokio::test]
    async fn crossing_the_configured_total_cap_is_413() {
        let caps = UploadCaps { max_files: 10, max_file_bytes: 1024, max_total_bytes: 15 };
        let (status, msg) =
            parse(&[("a.txt", 10), ("b.txt", 10)], caps).await.expect_err("over cap");
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
        assert!(msg.contains("15-byte total cap"), "{msg}");
    }

    #[tokio::test]
    async fn defaults_reproduce_the_previous_behaviour() {
        let caps = UploadCaps::default();
        assert_eq!(parse(&[("a.bin", 4 * 1024 * 1024)], caps).await, Ok(1));
        let (over_file, _) =
            parse(&[("a.bin", 5 * 1024 * 1024 + 1)], caps).await.expect_err("over cap");
        assert_eq!(over_file, StatusCode::PAYLOAD_TOO_LARGE);
        let eleven: Vec<(&str, usize)> = (0..11).map(|_| ("a.txt", 1)).collect();
        let (over_count, _) = parse(&eleven, caps).await.expect_err("over cap");
        assert_eq!(over_count, StatusCode::PAYLOAD_TOO_LARGE);
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
