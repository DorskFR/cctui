//! Shared attachment staging for adapters.
//!
//! Every adapter stages user-uploaded files under a per-session dir
//! ([`session_dir`]) and references the resulting absolute paths from the
//! turn/prompt. The staging logic — base64 decode, filename sanitization, 0700
//! dirs, 0600 files, collision-suffixing — lives here once so the adapters
//! cannot drift.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};

use cctui_proto::adapter::{BootstrapFile, BootstrapUploads};

/// The shared root, used when it is absent or already ours.
const SHARED_ROOT: &str = "/tmp/cctui-uploads";

/// Staged dirs older than this go even if their session still looks live: a
/// staged attachment is only needed while the turn referencing it is in flight.
pub const MAX_AGE: Duration = Duration::from_hours(24 * 7);

/// A dir younger than this is never swept — a spawn stages before the session
/// exists anywhere the sweep can see it.
pub const MIN_AGE: Duration = Duration::from_hours(1);

/// Root the staged per-session dirs live under, decided once per process.
///
/// `/tmp` is world-writable and shared, so the shared root is only used when it
/// is absent (we then create it 0700) or is a real directory this uid already
/// owns. Anything else — another user's dir, a symlink planted ahead of us — is
/// stepped around with a root under the user's runtime or cache dir, which no
/// other user can pre-create.
pub fn staging_root() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let shared = PathBuf::from(SHARED_ROOT);
        let private = [
            dirs::runtime_dir().map(|d| d.join("cctui-uploads")),
            dirs::cache_dir().map(|d| d.join("cctui").join("uploads")),
        ];
        let root = std::iter::once(Some(shared.clone()))
            .chain(private)
            .flatten()
            .find(|r| usable_root(r))
            .unwrap_or_else(|| shared.clone());
        if root != shared {
            tracing::warn!(root = %root.display(), "shared upload root unusable; staging privately");
        }
        tighten(&root);
        root
    })
}

/// A root this uid already owns may predate 0700 staging: close it up.
fn tighten(root: &Path) {
    #[cfg(unix)]
    if root.is_dir() {
        use std::os::unix::fs::PermissionsExt;
        if let Err(err) = std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700)) {
            tracing::warn!(%err, root = %root.display(), "could not restrict upload root");
        }
    }
    #[cfg(not(unix))]
    let _ = root;
}

/// Absolute per-session staging dir. Created on first write, not here.
#[must_use]
pub fn session_dir(session_id: &str) -> PathBuf {
    staging_root().join(session_id)
}

/// Whether `root` can be staged into: absent, or a non-symlink directory owned
/// by this uid.
fn usable_root(root: &Path) -> bool {
    match std::fs::symlink_metadata(root) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => true,
        Err(_) => false,
        Ok(meta) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                meta.is_dir() && meta.uid() == rustix::process::getuid().as_raw()
            }
            #[cfg(not(unix))]
            {
                meta.is_dir()
            }
        }
    }
}

/// Create `dir` and every missing parent with 0700, so a staged attachment is
/// never readable by another user on the machine.
fn create_private_dir(dir: &Path) -> Result<()> {
    #[cfg(unix)]
    let created = {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)
    };
    #[cfg(not(unix))]
    let created = std::fs::create_dir_all(dir);
    created.with_context(|| format!("creating upload dir {}", dir.display()))?;
    if !usable_root(dir) {
        anyhow::bail!("upload dir {} is not a directory owned by this user", dir.display());
    }
    Ok(())
}

/// Drop everything staged for `session_id`. Best-effort.
pub fn remove_session_dir(session_id: &str) {
    let dir = session_dir(session_id);
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => tracing::debug!(%session_id, "removed staged uploads"),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => tracing::warn!(%session_id, %err, "could not remove staged uploads"),
    }
}

/// Remove staged dirs in `root` whose session is absent from `live`, plus any
/// past `max_age` regardless. Returns how many were removed.
pub fn sweep_dir<S: std::hash::BuildHasher>(
    root: &Path,
    live: &HashSet<String, S>,
    now: SystemTime,
    max_age: Duration,
) -> std::io::Result<usize> {
    let mut removed = 0usize;
    for entry in std::fs::read_dir(root)? {
        let Ok(entry) = entry else { continue };
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Ok(modified) = meta.modified() else { continue };
        let age = now.duration_since(modified).unwrap_or_default();
        if !should_remove(name, age, live, max_age) {
            continue;
        }
        if std::fs::remove_dir_all(entry.path()).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

/// Whether the staged dir of `name`, last touched `age` ago, should go: past
/// `max_age` always, otherwise only when its session is unknown and it is old
/// enough that a spawn cannot still be staging into it.
fn should_remove<S: std::hash::BuildHasher>(
    name: &str,
    age: Duration,
    live: &HashSet<String, S>,
    max_age: Duration,
) -> bool {
    if age > max_age {
        return true;
    }
    let known = live.contains(name)
        || crate::configsweep::short_of(name).is_some_and(|short| live.contains(&short));
    !known && age >= MIN_AGE
}

/// Sweep the staging root against the sessions this machine still knows about.
pub fn sweep<S: std::hash::BuildHasher>(live: &HashSet<String, S>) {
    let root = staging_root();
    if !root.is_dir() {
        return;
    }
    match sweep_dir(root, live, SystemTime::now(), MAX_AGE) {
        Ok(0) => {}
        Ok(removed) => tracing::info!(removed, "swept staged upload dirs"),
        Err(err) => tracing::warn!(%err, path = %root.display(), "upload sweep failed"),
    }
}

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
/// ([`session_dir`]), returning the staged absolute paths.
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
    let dir = session_dir(session_id);
    create_private_dir(&dir)?;
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
        cctui_proto::util::write_private(&path, &bytes)
            .with_context(|| format!("writing upload {}", path.display()))?;
        paths.push(path.to_string_lossy().into_owned());
    }
    tracing::info!(%session_id, count = paths.len(), "staged uploaded files");
    Ok(paths)
}

/// Write `body` into the session's staging dir under `name`, 0600, returning
/// the absolute path. Same dir and permissions as an upload, so anything that
/// can read a staged attachment can read this.
pub fn stage_text(session_id: &str, name: &str, body: &str) -> Result<String> {
    let dir = session_dir(session_id);
    create_private_dir(&dir)?;
    let path = dir.join(name);
    cctui_proto::util::write_private(&path, body.as_bytes())
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(path.to_string_lossy().into_owned())
}

/// Resolve a non-colliding path in `dir` for `name`. If `dir/name` is free use
/// it; otherwise append `-1`, `-2`, … before the extension until a free path is
/// found.
fn unique_staging_path(dir: &Path, name: &str) -> PathBuf {
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
        let dir = session_dir(&session_id);

        let notes = dir.join("notes.txt");
        assert!(paths.contains(&notes.to_string_lossy().into_owned()));
        assert_eq!(std::fs::read_to_string(&notes).unwrap(), "hello world");
        let mode = std::fs::metadata(&notes).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "uploaded file must be 0600");

        // Traversal collapsed to the bare basename inside the staging dir.
        let evil = dir.join("evil");
        assert!(evil.exists(), "traversal name must be reduced to a basename in-dir");
        assert!(!staging_root().join("../../etc/evil").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stage_bootstrap_null_is_empty() {
        assert!(stage_bootstrap("sid", &serde_json::Value::Null).unwrap().is_empty());
    }

    #[test]
    fn stage_files_suffixes_name_collisions() {
        let session_id = format!("test-{}", uuid::Uuid::new_v4());
        let dir = session_dir(&session_id);

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
    fn stage_text_writes_one_private_file_and_overwrites_in_place() {
        use std::os::unix::fs::PermissionsExt;

        let session_id = format!("test-{}", uuid::Uuid::new_v4());
        let path = stage_text(&session_id, "context.md", "# one").expect("stage ok");
        assert!(path.ends_with(&format!("{session_id}/context.md")), "{path}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# one");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);

        // A relaunch restages the same name rather than accumulating copies.
        let again = stage_text(&session_id, "context.md", "# two").expect("stage ok");
        assert_eq!(again, path);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# two");

        remove_session_dir(&session_id);
    }

    #[cfg(unix)]
    #[test]
    fn the_session_staging_dir_is_0700_and_removable() {
        use std::os::unix::fs::PermissionsExt;

        let session_id = format!("test-{}", uuid::Uuid::new_v4());
        stage_text(&session_id, "context.md", "x").expect("stage ok");
        let dir = session_dir(&session_id);
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700, "staging dir must not be group/world readable");

        remove_session_dir(&session_id);
        assert!(!dir.exists(), "cleanup removes the staged dir");
        remove_session_dir(&session_id);
    }

    #[cfg(unix)]
    #[test]
    fn a_root_that_is_a_symlink_or_another_users_dir_is_not_usable() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("absent");
        assert!(usable_root(&missing), "an absent root is created by us");

        let real = tmp.path().join("real");
        std::fs::create_dir(&real).unwrap();
        assert!(usable_root(&real), "a dir we own is usable");

        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(!usable_root(&link), "a symlinked root must be refused");

        let file = tmp.path().join("file");
        std::fs::write(&file, "x").unwrap();
        assert!(!usable_root(&file), "a plain file is not a usable root");
    }

    #[test]
    fn sweep_keeps_live_and_young_dirs_and_drops_unknown_or_expired_ones() {
        let live: HashSet<String> =
            ["aaaa0001", "test-plain"].into_iter().map(str::to_owned).collect();
        let old = Duration::from_hours(6);
        let live_uuid = "aaaa0001-1111-2222-3333-444444444444";
        let other_uuid = "bbbb0002-1111-2222-3333-444444444444";

        assert!(!should_remove(live_uuid, old, &live, MAX_AGE), "a live session's dir stays");
        assert!(!should_remove("test-plain", old, &live, MAX_AGE), "match on the full id too");
        assert!(should_remove(other_uuid, old, &live, MAX_AGE), "an unknown session's dir goes");
        assert!(
            !should_remove(other_uuid, Duration::from_mins(1), &live, MAX_AGE),
            "a dir a spawn may still be staging into is never swept"
        );
        assert!(
            should_remove(live_uuid, MAX_AGE + Duration::from_hours(1), &live, MAX_AGE),
            "past the max age even a live session's dir goes"
        );
    }

    #[test]
    fn sweep_walks_only_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("stray.txt"), "x").unwrap();
        let young = root.join("dddd0004-1111-2222-3333-444444444444");
        std::fs::create_dir(&young).unwrap();

        let live: HashSet<String> = HashSet::new();
        assert_eq!(sweep_dir(root, &live, SystemTime::now(), MAX_AGE).unwrap(), 0);
        assert!(root.join("stray.txt").exists(), "non-directories are left alone");
        assert!(young.exists(), "a just-created dir survives");
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
