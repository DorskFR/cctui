use axum::Json;
use axum::extract::State;
use serde::Serialize;

use crate::routes::instance;
use crate::state::AppState;

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
    /// Newest upstream release, present **only** when strictly newer than
    /// `version` (see `update_check`). `null` when up to date, when the probe
    /// is disabled, or before its first answer.
    pub latest_version: Option<String>,
    /// Release page for `latest_version`.
    pub latest_url: Option<String>,
    /// Admin-set deployment label (`PUT /admin/instance`); `null` by default.
    pub instance_name: Option<String>,
}

pub async fn version(State(state): State<AppState>) -> Json<VersionInfo> {
    let commit_url = if GIT_HASH == "unknown" {
        REPO_URL.to_string()
    } else {
        format!("{REPO_URL}/commit/{GIT_HASH}")
    };
    let latest = state.update_check.newer().await;
    let instance_name = instance::read_name(&state.pool).await;
    Json(VersionInfo {
        version: VERSION,
        git_hash: GIT_HASH,
        repo_url: REPO_URL,
        commit_url,
        latest_version: latest.as_ref().map(|l| l.version.clone()),
        latest_url: latest.map(|l| l.url),
        instance_name,
    })
}
