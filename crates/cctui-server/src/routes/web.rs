use axum::Json;
use serde::Serialize;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const GIT_HASH: &str = env!("CCTUI_GIT_HASH");
const REPO_URL: &str = "https://github.com/DorskFR/cctui";

#[derive(Serialize, ts_rs::TS)]
#[ts(export)]
pub struct VersionInfo {
    pub version: &'static str,
    pub git_hash: &'static str,
    pub repo_url: &'static str,
    pub commit_url: String,
}

pub async fn version() -> Json<VersionInfo> {
    let commit_url = if GIT_HASH == "unknown" {
        REPO_URL.to_string()
    } else {
        format!("{REPO_URL}/commit/{GIT_HASH}")
    };
    Json(VersionInfo { version: VERSION, git_hash: GIT_HASH, repo_url: REPO_URL, commit_url })
}
