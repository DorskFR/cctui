use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
#[cfg(feature = "ts")]
use ts_rs::TS;
use uuid::Uuid;

use crate::adapter::PermissionMode;
use crate::api::SpawnContext;

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

/// What a cron fire does while the preset's previous run is still working.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum Overlap {
    #[default]
    Allow,
    Skip,
}

impl Overlap {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Skip => "skip",
        }
    }

    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "allow" => Some(Self::Allow),
            "skip" => Some(Self::Skip),
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
    pub account_id: Option<Uuid>,
    pub pool_id: Option<Uuid>,
    pub no_account: bool,
    pub dispatcher_id: Option<Uuid>,
    pub profile: Option<String>,
    pub external_sources: bool,
    pub fresh_secs: i32,
    pub on_stale: OnStale,
    pub max_batch: i32,
    pub cron: Option<String>,
    pub timezone: String,
    pub enabled: bool,
    pub overlap: Overlap,
    pub next_fire_at: Option<DateTime<Utc>>,
    pub last_fire_at: Option<DateTime<Utc>>,
    pub then_preset_id: Option<Uuid>,
    pub notify_url: Option<String>,
    /// Names of the sealed env every run gets; values are never returned.
    #[serde(default)]
    pub env_keys: Vec<String>,
    #[serde(default)]
    pub template: SpawnTemplate,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The rest of a draft's `SpawnRequest`, applied to every run a preset spawns.
///
/// `attachment_names` lists the files stored with `PUT /presets/{id}/attachments`
/// and is ignored on a preset write.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct SpawnTemplate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub label_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<SpawnContext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(type = "string | null", optional))]
    pub profile_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auto_archive: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachment_names: Vec<String>,
}

impl SpawnTemplate {
    /// Whether it sets anything a run would notice, stored files aside.
    #[must_use]
    pub fn is_blank(&self) -> bool {
        self.effort.is_none()
            && self.service_tier.is_none()
            && self.label_ids.is_empty()
            && self.context.as_ref().is_none_or(|c| c.items.is_empty() && !c.auto)
            && self.profile_id.is_none()
            && !self.auto_archive
    }
}

/// Body of `POST /sessions/{id}/schedule-repeat`: the draft becomes a
/// schedule with its whole payload, env and files included, and is removed.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct RepeatDraftRequest {
    /// The draft's session name when absent.
    #[serde(default)]
    pub name: Option<String>,
    pub cron: String,
    #[serde(default)]
    pub timezone: Option<String>,
    #[serde(default)]
    pub overlap: Option<Overlap>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

/// Body of `POST /presets` and `PUT /presets/{id}`.
///
/// Target is `machine_id` + `working_dir` + `adapter_id` (with optional
/// `model`, `permission_mode` and account pick), or `dispatcher_id` + `profile`.
/// The account pick is at most one of `account_id` / `pool_id` / `no_account`;
/// none of them lets the server elect the account with the most room.
/// `external_sources` lets n8n and webhook events reach the preset's lanes;
/// a machine target then needs `permission_mode` `ask` or `auto`.
///
/// Env is write-only: a name in `env_keys` without a value in `env` keeps its
/// stored value, a name in `env` is set, any other stored name is dropped.
/// With both absent the stored env is left as it is; an absent `template`
/// keeps the stored one.
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
    pub account_id: Option<Uuid>,
    #[serde(default)]
    pub pool_id: Option<Uuid>,
    #[serde(default)]
    pub no_account: bool,
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
    /// IANA name the cron is read in; UTC when absent.
    #[serde(default)]
    pub timezone: Option<String>,
    /// `false` pauses the preset's cron and its webhooks; absent means enabled.
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub overlap: Option<Overlap>,
    #[serde(default)]
    pub then_preset_id: Option<Uuid>,
    /// Container targets only: the completion webhook of every dispatch.
    #[serde(default)]
    pub notify_url: Option<String>,
    #[serde(default)]
    pub env_keys: Option<Vec<String>>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "{ [key in string]: string } | null"))]
    pub env: Option<EnvValues>,
    #[serde(default)]
    pub template: Option<SpawnTemplate>,
}

/// Env values on their way in; `Debug` never prints them.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EnvValues(pub std::collections::BTreeMap<String, String>);

impl std::fmt::Debug for EnvValues {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<{} secret(s) redacted>", self.0.len())
    }
}

/// Reply of `POST /presets/{id}/run`: the inbox event the run was queued as.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct PresetRun {
    pub event_id: Uuid,
    pub lane_id: Uuid,
}

#[cfg(test)]
mod tests {
    use super::{EnvValues, LaunchPresetRequest, OnStale, Overlap, SpawnTemplate};

    #[test]
    fn a_blank_template_serializes_to_nothing_and_ignores_stored_files() {
        assert_eq!(serde_json::to_value(SpawnTemplate::default()).unwrap(), serde_json::json!({}));
        let files =
            SpawnTemplate { attachment_names: vec!["a.md".into()], ..SpawnTemplate::default() };
        assert!(files.is_blank());
        let effort = SpawnTemplate { effort: Some("high".into()), ..SpawnTemplate::default() };
        assert!(!effort.is_blank());
        let back: SpawnTemplate = serde_json::from_value(serde_json::json!({
            "effort": "high", "label_ids": ["l1"], "context": {"items": ["notes"]},
            "auto_archive": true,
        }))
        .unwrap();
        assert_eq!(back.label_ids, ["l1"]);
        assert!(back.auto_archive && !back.is_blank());
    }

    #[test]
    fn env_values_never_reach_debug_output() {
        let req: LaunchPresetRequest = serde_json::from_value(serde_json::json!({
            "name": "n", "prompt": "p", "env_keys": ["TOKEN"], "env": {"TOKEN": "s3cr3t-v4lue"},
        }))
        .unwrap();
        assert_eq!(req.env.as_ref().map(|e| e.0.len()), Some(1));
        assert!(!format!("{req:?}").contains("s3cr3t-v4lue"));
        let absent: LaunchPresetRequest =
            serde_json::from_value(serde_json::json!({"name": "n", "prompt": "p"})).unwrap();
        assert!(absent.env.is_none() && absent.env_keys.is_none());
        assert_eq!(serde_json::to_value(EnvValues::default()).unwrap(), serde_json::json!({}));
    }

    #[test]
    fn on_stale_round_trips_its_wire_name() {
        for v in [OnStale::Followup, OnStale::New] {
            assert_eq!(OnStale::parse(v.as_str()), Some(v));
            assert_eq!(serde_json::to_value(v).unwrap(), v.as_str());
        }
        assert_eq!(OnStale::default(), OnStale::Followup);
        assert_eq!(OnStale::parse("reuse"), None);
    }

    #[test]
    fn overlap_round_trips_its_wire_name() {
        for v in [Overlap::Allow, Overlap::Skip] {
            assert_eq!(Overlap::parse(v.as_str()), Some(v));
            assert_eq!(serde_json::to_value(v).unwrap(), v.as_str());
        }
        assert_eq!(Overlap::default(), Overlap::Allow);
        assert_eq!(Overlap::parse("queue"), None);
    }
}
