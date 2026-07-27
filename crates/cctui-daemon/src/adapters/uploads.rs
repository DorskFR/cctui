//! Shared attachment staging for adapters.
//!
//! Both the claude-code and codex adapters stage user-uploaded files under a
//! per-session dir (`/tmp/cctui-uploads/<session-id>/`) and reference the
//! resulting absolute paths from the turn/prompt. The staging logic — base64
//! decode, filename sanitization, 0600 perms, collision-suffixing — lives here
//! once so the two adapters cannot drift. Claude consumes it at spawn +
//! mid-chat; codex at spawn + native image turn inputs.

use anyhow::{Context, Result};

use cctui_proto::adapter::{BootstrapFile, BootstrapUploads};

/// Decode the opaque [`cctui_proto::adapter::SessionSpec::bootstrap`] payload
/// and stage its uploads. A null/absent bootstrap stages nothing.
pub fn stage_bootstrap(session_id: &str, bootstrap: &serde_json::Value) -> Result<Vec<String>> {
    if bootstrap.is_null() {
        return Ok(Vec::new());
    }
    let parsed: BootstrapUploads =
        serde_json::from_value(bootstrap.clone()).context("decoding bootstrap uploads")?;
    stage_files(session_id, &parsed.uploads)
}

/// Decode + write a batch of uploaded files into the per-session staging dir
/// (`/tmp/cctui-uploads/<session_id>/`), returning the staged absolute paths.
///
/// Shared by spawn-time bootstrap uploads ([`stage_bootstrap`]) and mid-chat
/// attachments. Files are written 0600 (Unix). Name collisions —
/// against an existing staged file from an earlier upload in the same session —
/// are resolved by inserting a numeric suffix before the extension
/// (`report.pdf` → `report-1.pdf`) rather than overwriting, so a later
/// attachment never clobbers one the agent may still reference.
pub fn stage_files(session_id: &str, uploads: &[BootstrapFile]) -> Result<Vec<String>> {
    use base64::Engine;

    if uploads.is_empty() {
        return Ok(Vec::new());
    }
    let dir = std::path::Path::new("/tmp/cctui-uploads").join(session_id);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating upload dir {}", dir.display()))?;
    let mut paths = Vec::with_capacity(uploads.len());
    for file in uploads {
        // Defensive re-sanitize: the server already strips path separators, but
        // never trust a wire-supplied name when it becomes a filesystem path.
        let name = std::path::Path::new(&file.name)
            .file_name()
            .and_then(|s| s.to_str())
            .filter(|n| !n.is_empty() && *n != ".." && *n != ".")
            .ok_or_else(|| anyhow::anyhow!("unsafe upload filename: {:?}", file.name))?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(file.content_b64.as_bytes())
            .with_context(|| format!("base64-decoding upload {name}"))?;
        let path = unique_staging_path(&dir, name);
        std::fs::write(&path, &bytes)
            .with_context(|| format!("writing upload {}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .with_context(|| format!("chmod 0600 {}", path.display()))?;
        }
        paths.push(path.to_string_lossy().into_owned());
    }
    tracing::info!(%session_id, count = paths.len(), "staged uploaded files");
    Ok(paths)
}

/// Resolve a non-colliding path in `dir` for `name`. If `dir/name` is free use
/// it; otherwise append `-1`, `-2`, … before the extension until a free path is
/// found.
fn unique_staging_path(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let path = std::path::Path::new(name);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or(name);
    let ext = path.extension().and_then(|s| s.to_str());
    for n in 1u32.. {
        let alt = ext.map_or_else(|| format!("{stem}-{n}"), |ext| format!("{stem}-{n}.{ext}"));
        let candidate = dir.join(alt);
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!("exhausted u32 collision suffixes")
}

/// Whether a staged path is an image, by file extension.
///
/// Images are sent to codex as native `localImage` turn inputs so the model
/// sees the picture; every other file type keeps its path/text semantics.
/// Extensions mirror the set codex itself treats as inline images.
#[must_use]
pub fn is_image_path(path: &str) -> bool {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase);
    matches!(
        ext.as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tiff" | "tif" | "svg")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64(s: &str) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(s.as_bytes())
    }

    #[test]
    fn stage_bootstrap_writes_sanitized_0600_files() {
        use std::os::unix::fs::PermissionsExt;

        let session_id = format!("test-{}", uuid::Uuid::new_v4());
        // A normal name and a traversal attempt that must collapse to its basename.
        let bootstrap = serde_json::json!({
            "uploads": [
                { "name": "notes.txt", "content_b64": b64("hello world") },
                { "name": "../../etc/evil", "content_b64": b64("nope") },
            ]
        });

        let paths = stage_bootstrap(&session_id, &bootstrap).expect("stage ok");
        assert_eq!(paths.len(), 2);
        let dir = std::path::Path::new("/tmp/cctui-uploads").join(&session_id);

        let notes = dir.join("notes.txt");
        assert!(paths.contains(&notes.to_string_lossy().into_owned()));
        assert_eq!(std::fs::read_to_string(&notes).unwrap(), "hello world");
        let mode = std::fs::metadata(&notes).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "uploaded file must be 0600");

        // Traversal collapsed to the bare basename inside the staging dir.
        let evil = dir.join("evil");
        assert!(evil.exists(), "traversal name must be reduced to a basename in-dir");
        assert!(!std::path::Path::new("/tmp/cctui-uploads").join("../../etc/evil").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stage_bootstrap_null_is_empty() {
        assert!(stage_bootstrap("sid", &serde_json::Value::Null).unwrap().is_empty());
    }

    #[test]
    fn stage_files_suffixes_name_collisions() {
        let session_id = format!("test-{}", uuid::Uuid::new_v4());
        let dir = std::path::Path::new("/tmp/cctui-uploads").join(&session_id);

        // First upload stages report.pdf.
        let first = stage_files(
            &session_id,
            &[BootstrapFile { name: "report.pdf".into(), content_b64: b64("one") }],
        )
        .expect("stage ok");
        assert_eq!(first, vec![dir.join("report.pdf").to_string_lossy().into_owned()]);

        // A later upload with the same name must NOT overwrite — it gets a suffix.
        let second = stage_files(
            &session_id,
            &[
                BootstrapFile { name: "report.pdf".into(), content_b64: b64("two") },
                BootstrapFile { name: "report.pdf".into(), content_b64: b64("three") },
            ],
        )
        .expect("stage ok");
        assert_eq!(
            second,
            vec![
                dir.join("report-1.pdf").to_string_lossy().into_owned(),
                dir.join("report-2.pdf").to_string_lossy().into_owned(),
            ]
        );
        assert_eq!(std::fs::read_to_string(dir.join("report.pdf")).unwrap(), "one");
        assert_eq!(std::fs::read_to_string(dir.join("report-1.pdf")).unwrap(), "two");
        assert_eq!(std::fs::read_to_string(dir.join("report-2.pdf")).unwrap(), "three");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn image_detection_by_extension() {
        for p in ["/tmp/a.png", "/tmp/b.JPG", "shot.jpeg", "x.webp", "d.GIF"] {
            assert!(is_image_path(p), "{p} should be an image");
        }
        for p in ["/tmp/report.pdf", "notes.txt", "archive.tar.gz", "noext", "code.rs"] {
            assert!(!is_image_path(p), "{p} should not be an image");
        }
    }
}
