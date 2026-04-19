//! Build the expected-files manifest for this machine (CCT-68).
//!
//! Walks the Claude projects directory (reusing `archive::walk_project_dirs`)
//! and captures `(project_dir, session_id, size, mtime)` per `*.jsonl` file.
//! No hashing — cheap to run on every startup + 15-min tick.

use std::path::Path;

use cctui_proto::api::ManifestEntry;

use crate::archive;

#[must_use]
pub fn build(root: &Path) -> Vec<ManifestEntry> {
    let files = archive::walk_project_dirs(root);
    let mut out = Vec::with_capacity(files.len());
    for f in files {
        let Ok(meta) = std::fs::metadata(&f.abs_path) else { continue };
        let Ok(modified) = meta.modified() else { continue };
        let mtime: chrono::DateTime<chrono::Utc> = chrono::DateTime::<chrono::Utc>::from(modified);
        let size_bytes = i64::try_from(meta.len()).unwrap_or(i64::MAX);
        out.push(ManifestEntry {
            project_dir: f.project_dir,
            session_id: f.session_id,
            size_bytes,
            mtime,
        });
    }
    out
}
