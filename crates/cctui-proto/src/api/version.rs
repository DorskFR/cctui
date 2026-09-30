//! `GET /api/v1/version` and `/api/v1/version/changelog`.

use serde::{Deserialize, Serialize};

/// One release published since the running build: tag, page and Markdown notes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ReleaseNote {
    pub version: String,
    pub url: String,
    /// The release body as written on GitHub (Markdown); empty when the
    /// release has no description.
    pub body: String,
    /// ISO-8601 publication time as GitHub reports it, `null` for drafts.
    pub published_at: Option<String>,
}

#[derive(Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
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
    /// Admin-set deployment label (`PUT /admin/instance`); `null` by default,
    /// and always `null` for an unauthenticated caller.
    pub instance_name: Option<String>,
    /// Whether an admin can launch a self-update from here: a self-update
    /// machine is configured (settings or env). Everyone sees the flag, only
    /// admins get the button; the machine itself stays admin-only.
    pub self_update_ready: bool,
    /// Whether that machine has a deterministic update hook
    /// (`CCTUI_UPDATE_COMMAND`), so the update runs the operator's own command
    /// instead of a YOLO agent. Drives what the update modal promises.
    pub self_update_hook: bool,
    /// Effective per-upload caps, so the composer's pre-flight check matches
    /// what the server enforces without a second request. Defaults for an
    /// anonymous caller, like the rest of the deployment state.
    pub upload_caps: crate::api::uploads::UploadCaps,
}

#[derive(Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ChangelogResponse {
    pub version: &'static str,
    /// Releases published since `version`, newest first (capped server-side).
    /// Empty when up to date or before the probe's first answer.
    pub releases: Vec<ReleaseNote>,
}
