//! Daemon-binary manifest + download-proxy endpoints.
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

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
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

fn build_manifest(state: &AppState) -> DaemonManifest {
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
    DaemonManifest { version, assets }
}

fn manifest_etag(body: &[u8]) -> String {
    format!("\"{}\"", cctui_proto::util::sha256_hex(body))
}

fn if_none_match_hit(header_val: &str, etag: &str) -> bool {
    header_val.split(',').map(str::trim).any(|tag| tag == "*" || tag == etag)
}

fn manifest_response(body: Vec<u8>, if_none_match: Option<&str>) -> Response {
    let etag = manifest_etag(&body);
    if if_none_match.is_some_and(|v| if_none_match_hit(v, &etag)) {
        return (StatusCode::NOT_MODIFIED, [(header::ETAG, etag.as_str())]).into_response();
    }
    ([(header::CONTENT_TYPE, "application/json"), (header::ETAG, etag.as_str())], body)
        .into_response()
}

pub async fn daemon_manifest(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let body = serde_json::to_vec(&build_manifest(&state))
        .expect("DaemonManifest always serializes to JSON");
    let if_none_match = headers.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok());
    manifest_response(body, if_none_match)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn etag_is_stable_and_body_sensitive() {
        let body = br#"{"version":"0.3.31"}"#;
        assert_eq!(manifest_etag(body), manifest_etag(body));
        assert_ne!(manifest_etag(body), manifest_etag(br#"{"version":"0.3.32"}"#));
        assert!(manifest_etag(body).starts_with('"') && manifest_etag(body).ends_with('"'));
    }

    #[test]
    fn if_none_match_handles_lists_and_wildcard() {
        let etag = manifest_etag(b"x");
        assert!(if_none_match_hit(&etag, &etag));
        assert!(if_none_match_hit(&format!("\"other\", {etag}"), &etag));
        assert!(if_none_match_hit("*", &etag));
        assert!(!if_none_match_hit("\"nope\"", &etag));
    }

    #[test]
    fn matching_if_none_match_yields_304_without_body() {
        let body = br#"{"version":"0.3.31"}"#.to_vec();
        let etag = manifest_etag(&body);
        let res = manifest_response(body, Some(&etag));
        assert_eq!(res.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(res.headers().get(header::ETAG).unwrap().to_str().unwrap(), etag);
    }

    #[test]
    fn mismatch_yields_200_with_body_and_etag() {
        let body = br#"{"version":"0.3.31"}"#.to_vec();
        let etag = manifest_etag(&body);
        let res = manifest_response(body, Some("\"stale\""));
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers().get(header::ETAG).unwrap().to_str().unwrap(), etag);
    }
}
