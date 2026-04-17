use axum::Json;
use axum::http::header;
use axum::response::IntoResponse;
use serde::Serialize;

static INDEX_HTML: &str = include_str!("../../web/index.html");
const VERSION: &str = env!("CARGO_PKG_VERSION");
const GIT_HASH: &str = env!("CCTUI_GIT_HASH");
const REPO_URL: &str = "https://github.com/DorskFR/cctui";

#[derive(Serialize)]
pub struct VersionInfo {
    pub version: &'static str,
    pub git_hash: &'static str,
    pub repo_url: &'static str,
    pub commit_url: String,
}

pub async fn index() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8"), (header::CACHE_CONTROL, "no-cache")],
        INDEX_HTML,
    )
}

pub async fn version() -> Json<VersionInfo> {
    let commit_url = if GIT_HASH == "unknown" {
        REPO_URL.to_string()
    } else {
        format!("{REPO_URL}/commit/{GIT_HASH}")
    };
    Json(VersionInfo { version: VERSION, git_hash: GIT_HASH, repo_url: REPO_URL, commit_url })
}
