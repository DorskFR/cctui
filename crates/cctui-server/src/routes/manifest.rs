//! Daemon-binary manifest + download-proxy endpoints.
//!
//! `GET /api/v1/manifest/daemon` returns the server-known daemon version +
//! per-arch download and minisign-signature URLs, always on this server's own
//! origin so clients never send their credentials anywhere else. The daemon
//! ships in the same release as the TUI/server, so the version is simply the
//! server's own, and so is its release channel (a `-beta.N` version is beta).
//! A beta server answers `204 No Content` unless the caller opts in with
//! `?channel=beta`, so daemons that predate channels never see a beta.
//!
//! `GET /api/v1/daemon/binary/{target}` proxies the actual binary, and
//! `{target}.minisig` its detached signature. When the
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
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use cctui_proto::release_sig::Channel;
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
    pub channel: Channel,
    pub assets: Vec<DaemonAsset>,
}

#[derive(Debug, Serialize)]
pub struct DaemonAsset {
    pub target: &'static str,
    pub url: String,
    pub sig_url: String,
}

fn github_asset_url(version: &str, asset: &str) -> String {
    format!("https://github.com/{}/releases/download/v{version}/{asset}", repo())
}

fn build_manifest(state: &AppState) -> DaemonManifest {
    build_manifest_for(&state.config.external_url, env!("CARGO_PKG_VERSION"))
}

fn build_manifest_for(external_url: &str, version: &'static str) -> DaemonManifest {
    let base = external_url.trim_end_matches('/');
    let assets = TARGETS
        .iter()
        .map(|&target| DaemonAsset {
            target,
            url: format!("{base}/api/v1/daemon/binary/{target}"),
            sig_url: format!("{base}/api/v1/daemon/binary/{target}.minisig"),
        })
        .collect();
    DaemonManifest { version, channel: Channel::of_version(version), assets }
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

#[derive(Debug, Default, serde::Deserialize)]
pub struct ManifestQuery {
    #[serde(default)]
    channel: Option<String>,
}

/// Whether a caller asking with `requested` may be offered `version`. Absent or
/// unrecognised channels count as stable.
fn offered_to(version: &str, requested: Option<&str>) -> bool {
    let requested = requested.and_then(|c| c.parse::<Channel>().ok()).unwrap_or(Channel::Stable);
    requested == Channel::Beta || Channel::of_version(version) == Channel::Stable
}

pub async fn daemon_manifest(
    State(state): State<AppState>,
    Query(query): Query<ManifestQuery>,
    headers: HeaderMap,
) -> Response {
    if !offered_to(env!("CARGO_PKG_VERSION"), query.channel.as_deref()) {
        return StatusCode::NO_CONTENT.into_response();
    }
    let body = serde_json::to_vec(&build_manifest(&state))
        .expect("DaemonManifest always serializes to JSON");
    let if_none_match = headers.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok());
    manifest_response(body, if_none_match)
}

/// Map a `{target}` path segment to its GitHub release asset name: an arch
/// target, its `.minisig`, or `SHA256SUMS`.
fn asset_name_for(target: &str) -> Option<String> {
    if target == "SHA256SUMS" {
        Some("SHA256SUMS".to_string())
    } else if let Some(arch) = target.strip_suffix(".minisig")
        && TARGETS.contains(&arch)
    {
        Some(format!("cctui-daemon-{arch}.minisig"))
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
        // No PAT: redirect to the public GitHub asset. Clients drop their
        // Authorization header on the cross-origin hop; a private repo fails,
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
    fn manifest_urls_stay_on_the_server_origin() {
        let m = build_manifest_for("https://cctui.example.com/", "0.20.0");
        assert_eq!(m.assets.len(), TARGETS.len());
        for a in &m.assets {
            assert!(a.url.starts_with("https://cctui.example.com/api/v1/daemon/binary/"));
            assert_eq!(a.sig_url, format!("{}.minisig", a.url));
        }
    }

    #[test]
    fn manifest_channel_follows_the_server_version() {
        assert_eq!(build_manifest_for("https://s", "0.20.0").channel, Channel::Stable);
        assert_eq!(build_manifest_for("https://s", "0.21.0-beta.2").channel, Channel::Beta);
        let json = serde_json::to_value(build_manifest_for("https://s", "0.21.0-beta.2")).unwrap();
        assert_eq!(json["channel"], "beta");
        assert_eq!(json["version"], "0.21.0-beta.2");
    }

    #[test]
    fn beta_builds_are_only_offered_to_callers_that_opt_in() {
        let cases = [
            ("0.20.0", None, true),
            ("0.20.0", Some("stable"), true),
            ("0.20.0", Some("beta"), true),
            ("0.21.0-beta.1", None, false),
            ("0.21.0-beta.1", Some("stable"), false),
            ("0.21.0-beta.1", Some("nightly"), false),
            ("0.21.0-beta.1", Some(""), false),
            ("0.21.0-beta.1", Some("beta"), true),
            ("0.21.0-beta.1", Some("BETA"), true),
        ];
        for (version, requested, want) in cases {
            assert_eq!(offered_to(version, requested), want, "{version} asked as {requested:?}");
        }
    }

    #[test]
    fn asset_names_cover_signatures() {
        assert_eq!(asset_name_for("linux-amd64").as_deref(), Some("cctui-daemon-linux-amd64"));
        assert_eq!(
            asset_name_for("linux-amd64.minisig").as_deref(),
            Some("cctui-daemon-linux-amd64.minisig")
        );
        assert_eq!(asset_name_for("SHA256SUMS").as_deref(), Some("SHA256SUMS"));
        assert_eq!(asset_name_for("evil.minisig"), None);
        assert_eq!(asset_name_for("../x"), None);
    }

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
