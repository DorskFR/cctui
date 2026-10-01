//! Deserialize mirrors of the `/version` family.
//!
//! The server's own types are `Serialize`-only, so the client declares what it
//! reads. Unknown fields are ignored, which keeps an older TUI talking to a
//! newer server.

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct VersionInfo {
    pub version: String,
    pub git_hash: String,
    pub commit_url: String,
    /// Present only when strictly newer than `version`.
    #[serde(default)]
    pub latest_version: Option<String>,
    #[serde(default)]
    pub latest_url: Option<String>,
    /// Admin-set deployment label; always absent for an anonymous caller.
    #[serde(default)]
    pub instance_name: Option<String>,
    #[serde(default)]
    pub self_update_ready: bool,
    #[serde(default)]
    pub self_update_hook: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReleaseNote {
    pub version: String,
    pub url: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub published_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Changelog {
    pub version: String,
    #[serde(default)]
    pub releases: Vec<ReleaseNote>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SelfUpdateRun {
    pub id: uuid::Uuid,
    pub version: String,
    pub from_version: String,
    pub phase: cctui_proto::updatehook::UpdateHookPhase,
    pub done: bool,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub output_tail: Option<String>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Which path took the job: the machine's own hook, or an agent session.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum SelfUpdateLaunch {
    Hook {
        run_id: uuid::Uuid,
        version: String,
    },
    Agent {
        command_id: uuid::Uuid,
        #[serde(default)]
        session_id: Option<uuid::Uuid>,
        version: String,
        #[serde(default)]
        account: Option<String>,
    },
}

impl SelfUpdateLaunch {
    #[must_use]
    pub fn version(&self) -> &str {
        match self {
            Self::Hook { version, .. } | Self::Agent { version, .. } => version,
        }
    }

    /// Whether progress is a hook run to poll rather than a session to open.
    #[must_use]
    pub const fn is_hook(&self) -> bool {
        matches!(self, Self::Hook { .. })
    }
}
