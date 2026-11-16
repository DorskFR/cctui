use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
#[cfg(feature = "ts")]
use ts_rs::TS;
use uuid::Uuid;

use crate::adapter::PermissionMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum OnStale {
    #[default]
    Followup,
    New,
}

impl OnStale {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Followup => "followup",
            Self::New => "new",
        }
    }

    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "followup" => Some(Self::Followup),
            "new" => Some(Self::New),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct LaunchPreset {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub prompt: String,
    pub machine_id: Option<Uuid>,
    pub working_dir: Option<String>,
    pub adapter_id: Option<String>,
    pub model: Option<String>,
    pub permission_mode: Option<PermissionMode>,
    pub dispatcher_id: Option<Uuid>,
    pub profile: Option<String>,
    pub external_sources: bool,
    pub fresh_secs: i32,
    pub on_stale: OnStale,
    pub max_batch: i32,
    pub cron: Option<String>,
    pub next_fire_at: Option<DateTime<Utc>>,
    pub then_preset_id: Option<Uuid>,
    pub notify_url: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Body of `POST /presets` and `PUT /presets/{id}`.
///
/// Target is `machine_id` + `working_dir` + `adapter_id` (with optional
/// `model` and `permission_mode`), or `dispatcher_id` + `profile`.
/// `external_sources` lets n8n and webhook events reach the preset's lanes;
/// a machine target then needs `permission_mode` `ask` or `auto`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct LaunchPresetRequest {
    pub name: String,
    pub prompt: String,
    #[serde(default)]
    pub machine_id: Option<Uuid>,
    #[serde(default)]
    pub working_dir: Option<String>,
    #[serde(default)]
    pub adapter_id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub permission_mode: Option<PermissionMode>,
    #[serde(default)]
    pub dispatcher_id: Option<Uuid>,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub external_sources: bool,
    #[serde(default)]
    pub fresh_secs: Option<i32>,
    #[serde(default)]
    pub on_stale: Option<OnStale>,
    #[serde(default)]
    pub max_batch: Option<i32>,
    #[serde(default)]
    pub cron: Option<String>,
    #[serde(default)]
    pub then_preset_id: Option<Uuid>,
    /// Container targets only: the completion webhook of every dispatch.
    #[serde(default)]
    pub notify_url: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::OnStale;

    #[test]
    fn on_stale_round_trips_its_wire_name() {
        for v in [OnStale::Followup, OnStale::New] {
            assert_eq!(OnStale::parse(v.as_str()), Some(v));
            assert_eq!(serde_json::to_value(v).unwrap(), v.as_str());
        }
        assert_eq!(OnStale::default(), OnStale::Followup);
        assert_eq!(OnStale::parse("reuse"), None);
    }
}
