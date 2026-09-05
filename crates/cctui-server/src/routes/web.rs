use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::Serialize;

use crate::error::AppError;
use crate::routes::instance;
use crate::state::AppState;
use crate::update_check;

const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const GIT_HASH: &str = env!("CCTUI_GIT_HASH");
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
    /// Whether an admin can launch a self-update from here: a self-update
    /// machine is configured (settings or env). Everyone sees the flag, only
    /// admins get the button; the machine itself stays admin-only.
    pub self_update_ready: bool,
    /// Whether that machine has a deterministic update hook
    /// (`CCTUI_UPDATE_COMMAND`), so the update runs the operator's own command
    /// instead of a YOLO agent. Drives what the update modal promises.
    pub self_update_hook: bool,
}

#[derive(Serialize, ts_rs::TS)]
#[ts(export)]
pub struct ChangelogResponse {
    pub version: &'static str,
    /// Releases published since `version`, newest first (capped server-side).
    /// Empty when up to date or before the probe's first answer.
    pub releases: Vec<update_check::ReleaseNote>,
}

/// Release notes of every upstream release newer than this build, as the
/// background probe last saw them. No network call: the modal opens instantly.
pub async fn changelog(State(state): State<AppState>) -> Json<ChangelogResponse> {
    Json(ChangelogResponse { version: VERSION, releases: state.update_check.notes().await })
}

pub async fn version(State(state): State<AppState>) -> Json<VersionInfo> {
    Json(info(&state).await)
}

/// Probe upstream now instead of waiting out the 6h interval, then answer with
/// the same payload as `GET /version` so the caller can swap its cached copy.
///
/// Clicks inside [`update_check::MANUAL_COOLDOWN`] reuse the last answer rather
/// than querying GitHub again; a probe that fails surfaces as `502` so the
/// webui can say so instead of silently showing a stale "up to date".
pub async fn refresh_version(State(state): State<AppState>) -> Result<Json<VersionInfo>, AppError> {
    if !update_check::enabled_from_env() {
        return Err(AppError::new(
            StatusCode::CONFLICT,
            "update check is disabled on this server (CCTUI_UPDATE_CHECK=0)",
        ));
    }
    state
        .update_check
        .refresh(&state.http_client)
        .await
        .map_err(|e| AppError::new(StatusCode::BAD_GATEWAY, format!("update check failed: {e}")))?;
    Ok(Json(info(&state).await))
}

async fn info(state: &AppState) -> VersionInfo {
    let commit_url = if GIT_HASH == "unknown" {
        REPO_URL.to_string()
    } else {
        format!("{REPO_URL}/commit/{GIT_HASH}")
    };
    let latest = state.update_check.newer().await;
    let instance_name = instance::read_name(&state.pool).await;
    let self_update_target = instance::read_self_update_target(&state.pool).await.target;
    let self_update_ready = self_update_target.is_some();
    let self_update_hook = match self_update_target
        .and_then(|t| uuid::Uuid::parse_str(&t.machine_id).ok())
    {
        Some(machine) => crate::routes::update_hook::machine_has_hook(&state.pool, machine).await,
        None => false,
    };
    VersionInfo {
        version: VERSION,
        git_hash: GIT_HASH,
        repo_url: REPO_URL,
        commit_url,
        latest_version: latest.as_ref().map(|l| l.version.clone()),
        latest_url: latest.map(|l| l.url),
        instance_name,
        self_update_ready,
        self_update_hook,
    }
}
