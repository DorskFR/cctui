//! Serve one file off this machine for the webui (`DaemonFrameDown::ReadFile`).
//!
//! The path is one an agent linked in a message. The allow-list is the
//! image-post one (temp dirs + `$HOME`) widened by the session's working
//! directory; the path is canonicalised so a symlink pointing outside every
//! root is refused. Small files ride back inline, larger ones are PUT to the
//! blob store and answered by hash.

use std::path::{Path, PathBuf};

use base64::Engine;
use cctui_proto::media::sniff_media_type;
use cctui_proto::ws::{
    DaemonFrameUp, READ_FILE_INLINE_BYTES, READ_FILE_MAX_BYTES, ReadFileErrorKind, ReadFileOk,
};
use sha2::{Digest, Sha256};

use crate::client::ServerClient;

#[derive(Debug, PartialEq, Eq)]
pub struct Refused {
    pub kind: ReadFileErrorKind,
    pub message: String,
}

fn refused(kind: ReadFileErrorKind, message: impl Into<String>) -> Refused {
    Refused { kind, message: message.into() }
}

/// The image-post roots plus the session's working directory (when given),
/// each canonicalised so `starts_with` compares real paths.
#[must_use]
pub fn allowed_roots(cwd: Option<&str>) -> Vec<PathBuf> {
    let mut roots = crate::imagepost::default_allowed_roots();
    if let Some(cwd) = cwd
        && let Ok(real) = crate::git::expand_tilde(cwd).canonicalize()
    {
        roots.push(real);
    }
    roots
}

/// Expand `~`, canonicalise (following symlinks), and require a regular file
/// under one of `roots`. A symlink whose target escapes every root is refused
/// even when the link itself sits inside one.
pub fn resolve(path: &str, roots: &[PathBuf]) -> Result<PathBuf, Refused> {
    let expanded = crate::git::expand_tilde(path);
    if !expanded.is_absolute() {
        return Err(refused(ReadFileErrorKind::Denied, "path must be absolute"));
    }
    let real = match expanded.canonicalize() {
        Ok(p) => p,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(refused(ReadFileErrorKind::NotFound, format!("{path}: not found")));
        }
        Err(err) => {
            return Err(refused(ReadFileErrorKind::Io, format!("cannot resolve {path}: {err}")));
        }
    };
    if !roots.iter().any(|root| real.starts_with(root)) {
        return Err(refused(
            ReadFileErrorKind::Denied,
            format!("{path} is outside the allowed roots"),
        ));
    }
    let meta = std::fs::metadata(&real)
        .map_err(|err| refused(ReadFileErrorKind::Io, format!("cannot stat {path}: {err}")))?;
    if !meta.is_file() {
        return Err(refused(ReadFileErrorKind::Denied, format!("{path} is not a regular file")));
    }
    Ok(real)
}

/// Bytes of a resolved file, refusing anything over `min(max_bytes,
/// READ_FILE_MAX_BYTES)` before reading it.
pub fn read_capped(real: &Path, max_bytes: u64) -> Result<Vec<u8>, Refused> {
    let cap = max_bytes.min(READ_FILE_MAX_BYTES);
    let len = std::fs::metadata(real)
        .map_err(|err| refused(ReadFileErrorKind::Io, err.to_string()))?
        .len();
    if len > cap {
        return Err(refused(
            ReadFileErrorKind::TooLarge,
            format!("{} is {len} bytes; the cap is {cap} bytes", real.display()),
        ));
    }
    let bytes =
        std::fs::read(real).map_err(|err| refused(ReadFileErrorKind::Io, err.to_string()))?;
    if bytes.len() as u64 > cap {
        return Err(refused(
            ReadFileErrorKind::TooLarge,
            format!("{} grew past the cap", real.display()),
        ));
    }
    Ok(bytes)
}

fn file_name(real: &Path) -> String {
    real.file_name().and_then(|n| n.to_str()).unwrap_or("file").to_owned()
}

/// Resolve + read + (for large files) upload, producing the reply frame.
pub async fn handle(
    client: &ServerClient,
    machine_key: &str,
    request_id: uuid::Uuid,
    path: &str,
    max_bytes: u64,
    cwd: Option<&str>,
) -> DaemonFrameUp {
    match read(client, machine_key, path, max_bytes, cwd).await {
        Ok(file) => DaemonFrameUp::ReadFileResult {
            request_id,
            ok: true,
            file: Some(file),
            error_kind: None,
            error: None,
        },
        Err(Refused { kind, message }) => {
            tracing::warn!(%path, ?kind, %message, "read-file refused");
            DaemonFrameUp::ReadFileResult {
                request_id,
                ok: false,
                file: None,
                error_kind: Some(kind),
                error: Some(message),
            }
        }
    }
}

async fn read(
    client: &ServerClient,
    machine_key: &str,
    path: &str,
    max_bytes: u64,
    cwd: Option<&str>,
) -> Result<ReadFileOk, Refused> {
    let roots = allowed_roots(cwd);
    let real = resolve(path, &roots)?;
    let bytes = read_capped(&real, max_bytes)?;
    let name = file_name(&real);
    let size = bytes.len() as u64;
    let sha256 = hex::encode(Sha256::digest(&bytes));
    let media_type = sniff_media_type(&name, &bytes[..bytes.len().min(8192)]);
    if size <= READ_FILE_INLINE_BYTES {
        return Ok(ReadFileOk {
            name,
            size,
            sha256,
            media_type: Some(media_type.to_owned()),
            data: Some(base64::engine::general_purpose::STANDARD.encode(&bytes)),
            blob_hash: None,
        });
    }
    client
        .put_blob(machine_key, &sha256, bytes, Some(media_type))
        .await
        .map_err(|err| refused(ReadFileErrorKind::Io, format!("blob upload failed: {err}")))?;
    Ok(ReadFileOk {
        name,
        size,
        sha256: sha256.clone(),
        media_type: Some(media_type.to_owned()),
        data: None,
        blob_hash: Some(sha256),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roots_of(dir: &Path) -> Vec<PathBuf> {
        vec![dir.canonicalize().unwrap()]
    }

    #[test]
    fn regular_file_under_a_root_resolves() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("report.md");
        std::fs::write(&f, "# hi").unwrap();
        let real = resolve(f.to_str().unwrap(), &roots_of(dir.path())).unwrap();
        assert_eq!(real, f.canonicalize().unwrap());
        assert_eq!(read_capped(&real, READ_FILE_MAX_BYTES).unwrap(), b"# hi");
    }

    #[test]
    fn outside_roots_relative_and_directories_are_denied() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let f = other.path().join("x.txt");
        std::fs::write(&f, "x").unwrap();
        let roots = roots_of(dir.path());
        assert_eq!(
            resolve(f.to_str().unwrap(), &roots).unwrap_err().kind,
            ReadFileErrorKind::Denied
        );
        assert_eq!(resolve("relative/x.txt", &roots).unwrap_err().kind, ReadFileErrorKind::Denied);
        assert_eq!(
            resolve(dir.path().to_str().unwrap(), &roots).unwrap_err().kind,
            ReadFileErrorKind::Denied
        );
        assert_eq!(
            resolve(dir.path().join("missing").to_str().unwrap(), &roots).unwrap_err().kind,
            ReadFileErrorKind::NotFound
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escaping_the_root_is_denied() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("passwd");
        std::os::unix::fs::symlink("/etc/passwd", &link).unwrap();
        let err = resolve(link.to_str().unwrap(), &roots_of(dir.path())).unwrap_err();
        assert_eq!(err.kind, ReadFileErrorKind::Denied);

        let dotdot = dir.path().join("sub/../../../../etc/passwd");
        std::fs::create_dir_all(dir.path().join("sub")).unwrap();
        let err = resolve(dotdot.to_str().unwrap(), &roots_of(dir.path())).unwrap_err();
        assert_eq!(err.kind, ReadFileErrorKind::Denied);
    }

    #[test]
    fn cwd_widens_the_allow_list() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("out.txt");
        std::fs::write(&f, "x").unwrap();
        let roots = allowed_roots(Some(dir.path().to_str().unwrap()));
        assert!(roots.contains(&dir.path().canonicalize().unwrap()));
        assert!(resolve(f.to_str().unwrap(), &roots).is_ok());
        let none = allowed_roots(Some("/definitely/not/a/dir"));
        assert_eq!(none, crate::imagepost::default_allowed_roots());
    }

    #[test]
    fn size_cap_is_enforced_before_reading() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("big.bin");
        std::fs::write(&f, vec![0u8; 1024]).unwrap();
        let real = f.canonicalize().unwrap();
        let err = read_capped(&real, 1023).unwrap_err();
        assert_eq!(err.kind, ReadFileErrorKind::TooLarge);
        assert!(read_capped(&real, 1024).is_ok());
        assert!(read_capped(&real, u64::MAX).is_ok(), "caller cap clamps to READ_FILE_MAX_BYTES");
    }

    #[tokio::test]
    async fn handle_reports_refusals_as_error_frames() {
        let client = ServerClient::new("http://127.0.0.1:9");
        let up = handle(&client, "k", uuid::Uuid::nil(), "/proc/self/status", 10, None).await;
        match up {
            DaemonFrameUp::ReadFileResult { ok, error_kind, file, .. } => {
                assert!(!ok);
                assert_eq!(error_kind, Some(ReadFileErrorKind::Denied));
                assert!(file.is_none());
            }
            other => panic!("unexpected frame {other:?}"),
        }
    }

    #[tokio::test]
    async fn handle_inlines_small_files_with_hash_and_type() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("note.md");
        std::fs::write(&f, "# note").unwrap();
        let client = ServerClient::new("http://127.0.0.1:9");
        let cwd = dir.path().to_str().unwrap();
        let up =
            handle(&client, "k", uuid::Uuid::nil(), f.to_str().unwrap(), 1 << 20, Some(cwd)).await;
        let DaemonFrameUp::ReadFileResult { ok: true, file: Some(file), .. } = up else {
            panic!("expected ok frame, got {up:?}");
        };
        assert_eq!(file.name, "note.md");
        assert_eq!(file.size, 6);
        assert_eq!(file.sha256, hex::encode(Sha256::digest(b"# note")));
        assert_eq!(file.media_type.as_deref(), Some("text/markdown; charset=utf-8"));
        assert_eq!(file.data.as_deref(), Some("IyBub3Rl"));
        assert!(file.blob_hash.is_none());
    }
}
