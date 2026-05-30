//! Daemon-binary manifest + download-proxy endpoints (CCT-88, CCT-117).
//!
//! `GET /api/v1/manifest/daemon` returns the server-known daemon version +
//! per-arch download URLs. The daemon ships in the same release as the
//! TUI/server, so the version is simply the server's own.
//!
//! `GET /api/v1/daemon/binary/{target}` proxies the actual binary. When the
//! releases repo is private its assets aren't publicly downloadable, so if
//! the server is configured with a GitHub PAT
//! (`CCTUI_GITHUB_TOKEN`/`GH_TOKEN`) it streams the asset itself — clients
//! never need a token and a private releases repo stays private. Without a
//! PAT it falls back to a 302 to the raw GitHub URL (which fails for a
//! private repo — the intended graceful no-op for selfupdate until a token
//! is provided).
//!
//! Routing every version-check / selfupdate / download through these
//! endpoints makes the server the single channel for daemon distribution.

use axum::Json;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Serialize;

use crate::state::AppState;

/// GitHub `owner/repo` to fetch daemon release assets from. Defaults to the
/// upstream repo; set `CCTUI_REPO` to point at a fork's releases.
const DEFAULT_REPO: &str = "DorskFR/cctui";

fn repo() -> String {
    std::env::var("CCTUI_REPO").unwrap_or_else(|_| DEFAULT_REPO.to_string())
}

/// Targets we publish daemon binaries for (see `.github/workflows/release.yml`).
const TARGETS: [&str; 3] = ["linux-amd64", "linux-arm64", "darwin-arm64"];

#[derive(Debug, Serialize)]
pub struct DaemonManifest {
    pub version: &'static str,
    pub assets: Vec<DaemonAsset>,
}

#[derive(Debug, Serialize)]
pub struct DaemonAsset {
    pub target: &'static str,
    pub url: String,
}

fn github_asset_url(version: &str, asset: &str) -> String {
    format!("https://github.com/{}/releases/download/v{version}/{asset}", repo())
}

pub async fn daemon_manifest(State(state): State<AppState>) -> Json<DaemonManifest> {
    let version = env!("CARGO_PKG_VERSION");
    // With a PAT, point clients at our proxy so the private-repo binary is
    // served by us; otherwise hand back the raw GitHub URLs.
    let proxying = state.config.github_token.is_some();
    let base = state.config.external_url.trim_end_matches('/');
    let assets = TARGETS
        .iter()
        .map(|&target| {
            let url = if proxying {
                format!("{base}/api/v1/daemon/binary/{target}")
            } else {
                github_asset_url(version, &format!("cctui-daemon-{target}"))
            };
            DaemonAsset { target, url }
        })
        .collect();
    Json(DaemonManifest { version, assets })
}

/// Map a `{target}` path segment to its GitHub release asset name.
/// Accepts the three arch targets plus `SHA256SUMS` (for selfupdate
/// checksum verification).
fn asset_name_for(target: &str) -> Option<String> {
    if target == "SHA256SUMS" {
        Some("SHA256SUMS".to_string())
    } else if TARGETS.contains(&target) {
        Some(format!("cctui-daemon-{target}"))
    } else {
        None
    }
}

#[derive(serde::Deserialize)]
struct GhAsset {
    name: String,
    id: u64,
}

#[derive(serde::Deserialize)]
struct GhRelease {
    assets: Vec<GhAsset>,
}

/// `GET /api/v1/daemon/binary/{target}` — stream the release asset (PAT set)
/// or redirect to GitHub (no PAT).
pub async fn download_daemon_binary(
    State(state): State<AppState>,
    Path(target): Path<String>,
) -> Result<Response, (StatusCode, String)> {
    let version = env!("CARGO_PKG_VERSION");
    let asset = asset_name_for(&target)
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("unknown target: {target}")))?;

    let Some(token) = state.config.github_token.as_deref() else {
        // No PAT: hand back the raw GitHub URL. Fails for a private repo,
        // which is the intended graceful degradation.
        return Ok(Redirect::temporary(&github_asset_url(version, &asset)).into_response());
    };

    let client = reqwest::Client::new();
    // 1) Resolve the release for this version to find the asset id.
    let rel_url = format!("https://api.github.com/repos/{}/releases/tags/v{version}", repo());
    let rel: GhRelease = client
        .get(&rel_url)
        .bearer_auth(token)
        .header(header::USER_AGENT, "cctui-server")
        .header(header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("github release lookup failed: {e}")))?
        .json()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("github release parse failed: {e}")))?;

    let asset_id = rel.assets.iter().find(|a| a.name == asset).map(|a| a.id).ok_or_else(|| {
        (StatusCode::NOT_FOUND, format!("asset {asset} not in release v{version}"))
    })?;

    // 2) Stream the asset bytes. The GitHub asset API 302-redirects to a
    //    signed S3 URL; reqwest strips the Authorization header on the
    //    cross-host redirect, so the bearer token never leaks to S3.
    let upstream = client
        .get(format!("https://api.github.com/repos/{}/releases/assets/{asset_id}", repo()))
        .bearer_auth(token)
        .header(header::USER_AGENT, "cctui-server")
        .header(header::ACCEPT, "application/octet-stream")
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("github asset download failed: {e}")))?;

    let body = Body::from_stream(upstream.bytes_stream());
    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (header::CONTENT_DISPOSITION, &format!("attachment; filename={asset}")),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        body,
    )
        .into_response())
}
