//! Periodic archive sync (CCT-362) — opt-in, rsync-style.
//!
//! When `archive.enabled = true` in `daemon.toml`, on an interval the daemon
//! walks the local Claude session trees and mirrors each transcript to the
//! server archive that already exists (`HEAD`/`PUT /api/v1/archive/...` +
//! `POST /api/v1/archive/manifest`). Authentication is the machine key — the
//! same credential the WS uses — so the server resolves `machine_id` from it.
//!
//! Per file: `HEAD …?sha256=<local>` to skip unchanged (server replies 204 on a
//! match), else `PUT` the byte-exact body with an `X-CCTUI-SHA256` guard, then
//! `POST` the full expected manifest so the server's status derivation works.
//!
//! Scope (v1): top-level Claude transcripts
//! `~/.claude/projects/<encoded-cwd>/<session>.jsonl`. Codex rollouts and
//! per-subagent files need the harness-namespaced archive paths (the CCT-362
//! "prerequisite") and are deliberately deferred — the current server routes
//! are keyed `(project_dir, session_id)` single-segment, which only the
//! top-level Claude layout fits. Tracked as follow-up.

use std::path::{Path, PathBuf};
use std::time::Duration;

use cctui_proto::api::{ManifestEntry, ManifestPostRequest};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

/// One discovered local transcript.
struct LocalFile {
    project_dir: String,
    session_id: String,
    path: PathBuf,
    size_bytes: i64,
    mtime: chrono::DateTime<chrono::Utc>,
}

/// Run the sync loop until cancelled. `projects_root` is `~/.claude/projects`.
pub async fn run(
    base_url: String,
    machine_key: String,
    projects_root: PathBuf,
    interval: Duration,
    cancel: CancellationToken,
) {
    let http = reqwest::Client::new();
    let base = base_url.trim_end_matches('/').to_owned();
    tracing::info!(interval_secs = interval.as_secs(), root = %projects_root.display(), "archive sync enabled");

    loop {
        if let Err(e) = sync_once(&http, &base, &machine_key, &projects_root).await {
            tracing::warn!("archive sync pass failed: {e}");
        }
        tokio::select! {
            () = cancel.cancelled() => {
                tracing::info!("archive sync stopping");
                return;
            }
            () = tokio::time::sleep(interval) => {}
        }
    }
}

async fn sync_once(
    http: &reqwest::Client,
    base: &str,
    machine_key: &str,
    projects_root: &Path,
) -> anyhow::Result<()> {
    let files = discover(projects_root);
    if files.is_empty() {
        return Ok(());
    }

    let mut uploaded = 0usize;
    for f in &files {
        match upload_if_needed(http, base, machine_key, f).await {
            Ok(true) => uploaded += 1,
            Ok(false) => {}
            Err(e) => tracing::warn!(session = %f.session_id, "archive upload failed: {e}"),
        }
    }

    // POST the full expected manifest so server status derivation is correct.
    let entries: Vec<ManifestEntry> = files
        .iter()
        .map(|f| ManifestEntry {
            project_dir: f.project_dir.clone(),
            session_id: f.session_id.clone(),
            size_bytes: f.size_bytes,
            mtime: f.mtime,
        })
        .collect();
    let url = format!("{base}/api/v1/archive/manifest");
    let resp = http
        .post(&url)
        .bearer_auth(machine_key)
        .json(&ManifestPostRequest { entries })
        .send()
        .await?;
    if !resp.status().is_success() {
        tracing::warn!(status = %resp.status(), "archive manifest post non-2xx");
    }

    tracing::info!(total = files.len(), uploaded, "archive sync pass complete");
    Ok(())
}

/// `HEAD` to check the server already has this exact sha; if not, `PUT` it.
/// Returns whether an upload happened.
async fn upload_if_needed(
    http: &reqwest::Client,
    base: &str,
    machine_key: &str,
    f: &LocalFile,
) -> anyhow::Result<bool> {
    let body = tokio::fs::read(&f.path).await?;
    let sha = hex_sha256(&body);

    let head_url = format!("{base}/api/v1/archive/{}/{}?sha256={sha}", f.project_dir, f.session_id);
    let head = http.head(&head_url).bearer_auth(machine_key).send().await?;
    if head.status() == reqwest::StatusCode::NO_CONTENT {
        return Ok(false); // server already has this exact content
    }

    let put_url = format!("{base}/api/v1/archive/{}/{}", f.project_dir, f.session_id);
    let put = http
        .put(&put_url)
        .bearer_auth(machine_key)
        .header("X-CCTUI-SHA256", &sha)
        .body(body)
        .send()
        .await?;
    if !put.status().is_success() {
        anyhow::bail!("PUT {} -> {}", put_url, put.status());
    }
    Ok(true)
}

/// Walk `<root>/<encoded-cwd>/<session>.jsonl`. Skips the `.partial` temp files
/// and anything not directly under a project dir.
fn discover(root: &Path) -> Vec<LocalFile> {
    let mut out = Vec::new();
    let Ok(projects) = std::fs::read_dir(root) else {
        return out;
    };
    for proj in projects.flatten() {
        if !proj.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let project_dir = proj.file_name().to_string_lossy().into_owned();
        if !valid_name(&project_dir) {
            continue;
        }
        let Ok(sessions) = std::fs::read_dir(proj.path()) else {
            continue;
        };
        for sess in sessions.flatten() {
            let name = sess.file_name().to_string_lossy().into_owned();
            let Some(session_id) = name.strip_suffix(".jsonl") else {
                continue; // dirs (subagents/) and non-jsonl skipped (v1)
            };
            if !valid_name(session_id) {
                continue;
            }
            let Ok(meta) = sess.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            let mtime = meta
                .modified()
                .ok()
                .map_or_else(chrono::Utc::now, chrono::DateTime::<chrono::Utc>::from);
            out.push(LocalFile {
                project_dir: project_dir.clone(),
                session_id: session_id.to_owned(),
                path: sess.path(),
                size_bytes: i64::try_from(meta.len()).unwrap_or(i64::MAX),
                mtime,
            });
        }
    }
    out
}

fn hex_sha256(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    let digest = h.finalize();
    let mut s = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Mirror the server's `valid_name`: reject empties, dots, and separators so we
/// never build a path the server will 400.
fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && !s.contains('/')
        && !s.contains('\\')
        && !s.contains('\0')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_finds_top_level_transcripts_only() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let proj = root.join("-home-u-proj");
        std::fs::create_dir_all(proj.join("sess-1/subagents")).unwrap();
        std::fs::write(proj.join("sess-1.jsonl"), b"{}\n").unwrap();
        std::fs::write(proj.join("sess-1/subagents/agent-1.jsonl"), b"{}\n").unwrap();
        std::fs::write(proj.join("notes.txt"), b"x").unwrap();

        let found = discover(root);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].project_dir, "-home-u-proj");
        assert_eq!(found[0].session_id, "sess-1");
    }

    #[test]
    fn hex_sha256_matches_known_vector() {
        // sha256("") well-known digest.
        assert_eq!(
            hex_sha256(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
