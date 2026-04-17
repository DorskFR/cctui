//! Walk `$CLAUDE_PROJECTS_DIR/<project>/*.jsonl`, sha256, HEAD-then-PUT to the
//! server. Port of `channel/src/archive.ts`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use sha2::{Digest, Sha256};

use crate::bridge::{ArchiveState, Bridge};

#[derive(Debug, Clone)]
pub struct ProjectFile {
    pub abs_path: PathBuf,
    pub project_dir: String,
    pub session_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadOutcome {
    Skipped,
    Uploaded,
    Failed,
}

#[must_use]
pub fn walk_project_dirs(root: &Path) -> Vec<ProjectFile> {
    let Ok(projects) = std::fs::read_dir(root) else { return Vec::new() };
    let mut out = Vec::new();
    for proj in projects.flatten() {
        let proj_path = proj.path();
        if !proj_path.is_dir() {
            continue;
        }
        let project_dir = proj.file_name().to_string_lossy().into_owned();
        let Ok(entries) = std::fs::read_dir(&proj_path) else { continue };
        for entry in entries.flatten() {
            let abs_path = entry.path();
            if !abs_path.is_file() {
                continue;
            }
            let Some(name) = abs_path.file_name().and_then(|n| n.to_str()) else { continue };
            if !name.ends_with(".jsonl") {
                continue;
            }
            let session_id = name.trim_end_matches(".jsonl").to_string();
            out.push(ProjectFile { abs_path, project_dir: project_dir.clone(), session_id });
        }
    }
    out
}

pub fn compute_file_sha256(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex::encode(h.finalize()))
}

#[derive(Default)]
pub struct ArchiveCache {
    inner: Mutex<HashMap<PathBuf, String>>,
}

impl ArchiveCache {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn get(&self, path: &Path) -> Option<String> {
        self.inner.lock().ok()?.get(path).cloned()
    }

    fn set(&self, path: &Path, sha: String) {
        if let Ok(mut g) = self.inner.lock() {
            g.insert(path.to_path_buf(), sha);
        }
    }
}

pub async fn upload_if_changed(
    bridge: &Bridge,
    cache: &ArchiveCache,
    file: &ProjectFile,
) -> UploadOutcome {
    let sha = match compute_file_sha256(&file.abs_path) {
        Ok(sha) => sha,
        Err(err) => {
            tracing::error!(path = %file.abs_path.display(), error = %err, "hash failed");
            return UploadOutcome::Failed;
        }
    };
    if cache.get(&file.abs_path).as_deref() == Some(sha.as_str()) {
        return UploadOutcome::Skipped;
    }
    let state = match bridge.head_archive(&file.project_dir, &file.session_id, &sha).await {
        Ok(s) => s,
        Err(err) => {
            tracing::error!(error = %err, "HEAD archive failed");
            return UploadOutcome::Failed;
        }
    };
    if matches!(state, ArchiveState::Present) {
        cache.set(&file.abs_path, sha);
        return UploadOutcome::Skipped;
    }
    match bridge.put_archive(&file.project_dir, &file.session_id, &file.abs_path, &sha).await {
        Ok(()) => {
            cache.set(&file.abs_path, sha);
            UploadOutcome::Uploaded
        }
        Err(err) => {
            tracing::error!(session = %file.session_id, error = %err, "PUT archive failed");
            UploadOutcome::Failed
        }
    }
}
