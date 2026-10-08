//! Harness auto-update: the policy pushed to daemons and the heartbeat report.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
#[cfg(feature = "ts")]
use ts_rs::TS;

pub const HARNESS_CLAUDE_CODE: &str = "claude-code";
pub const HARNESS_CODEX: &str = "codex";
pub const KNOWN_HARNESSES: &[&str] = &[HARNESS_CLAUDE_CODE, HARNESS_CODEX];

pub const DEFAULT_INTERVAL_HOURS: u32 = 24;
pub const MIN_INTERVAL_HOURS: u32 = 1;

const fn default_interval_hours() -> u32 {
    DEFAULT_INTERVAL_HOURS
}

fn default_harnesses() -> Vec<String> {
    KNOWN_HARNESSES.iter().map(|h| (*h).to_owned()).collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct HarnessUpdatePolicy {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_interval_hours")]
    pub interval_hours: u32,
    #[serde(default = "default_harnesses")]
    pub harnesses: Vec<String>,
}

impl Default for HarnessUpdatePolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_hours: DEFAULT_INTERVAL_HOURS,
            harnesses: default_harnesses(),
        }
    }
}

impl HarnessUpdatePolicy {
    /// Clamp the interval and drop unknown or duplicate harness ids.
    #[must_use]
    pub fn normalized(mut self) -> Self {
        self.interval_hours = self.interval_hours.max(MIN_INTERVAL_HOURS);
        let mut seen = Vec::new();
        for h in self.harnesses {
            if KNOWN_HARNESSES.contains(&h.as_str()) && !seen.contains(&h) {
                seen.push(h);
            }
        }
        self.harnesses = seen;
        self
    }

    /// Machine override over instance default; off when neither is set.
    #[must_use]
    pub fn resolve(instance: Option<&Self>, machine: Option<&Self>) -> Self {
        machine.or(instance).cloned().unwrap_or_default().normalized()
    }

    #[must_use]
    pub fn covers(&self, harness: &str) -> bool {
        self.enabled && self.harnesses.iter().any(|h| h == harness)
    }
}

/// CLI version on disk and the version of the long-lived process serving
/// sessions (`claude daemon`, `codex app-server daemon`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct HarnessVersion {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daemon: Option<String>,
}

/// Versions per harness, keyed by adapter id (`claude-code`, `codex`,
/// `gemini`, …). A daemon from before the map sent two fixed fields; its
/// `claude_code` key is read as `claude-code`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
#[serde(transparent)]
pub struct HarnessVersions(pub BTreeMap<String, HarnessVersion>);

impl HarnessVersions {
    #[must_use]
    pub fn get(&self, harness: &str) -> Option<&HarnessVersion> {
        self.0.get(harness)
    }

    pub fn insert(&mut self, harness: &str, version: HarnessVersion) {
        self.0.insert(harness.to_owned(), version);
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &HarnessVersion)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v))
    }
}

impl<'de> Deserialize<'de> for HarnessVersions {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut map: BTreeMap<String, Option<HarnessVersion>> =
            BTreeMap::deserialize(deserializer)?;
        if let Some(legacy) = map.remove("claude_code")
            && !map.contains_key(HARNESS_CLAUDE_CODE)
        {
            map.insert(HARNESS_CLAUDE_CODE.to_owned(), legacy);
        }
        Ok(Self(map.into_iter().filter_map(|(k, v)| v.map(|v| (k, v))).collect()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct HarnessOutcome {
    pub harness: String,
    /// `updated a→b`, `up to date`, `failed: …`, `deferred: busy`, `not installed`.
    pub outcome: String,
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub at: chrono::DateTime<chrono::Utc>,
}

/// Result of the daemon's codex sandbox probe.
///
/// Codex's Linux sandbox is bubblewrap, and host policy can break it outright:
/// on Ubuntu 24.04+ `kernel.apparmor_restrict_unprivileged_userns=1` with no
/// bwrap `AppArmor` profile makes every sandboxed command fail. Codex reports the
/// sandbox as healthy in that state, so cctui probes it itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CodexSandbox {
    /// A sandboxed command ran.
    Ok,
    /// Recognized as the host refusing unprivileged user namespaces, which an
    /// `AppArmor` profile for `bwrap` fixes.
    UsernsDenied { detail: String },
    /// The probe failed for some other reason.
    Unknown { detail: String },
}

impl CodexSandbox {
    #[must_use]
    pub const fn is_ok(&self) -> bool {
        matches!(self, Self::Ok)
    }

    #[must_use]
    pub fn detail(&self) -> Option<&str> {
        match self {
            Self::Ok => None,
            Self::UsernsDenied { detail } | Self::Unknown { detail } => Some(detail),
        }
    }
}

/// The message a user needs when a spawn is refused or a badge is shown. Kept
/// here rather than in the webui because the daemon puts it on the failed
/// `CommandResult` too.
pub const CODEX_SANDBOX_FIX: &str = "codex's sandbox (bubblewrap) cannot start on this host: AppArmor blocks unprivileged user \
     namespaces for bwrap. Install an AppArmor profile for bwrap, or set \
     kernel.apparmor_restrict_unprivileged_userns=0. See docs/codex-sandbox.md";

/// Heartbeat block. `policy` echoes what the daemon currently holds so the
/// server resends [`crate::ws::DaemonFrameDown::HarnessUpdatePolicy`] only on
/// a difference, and only to a daemon that can parse it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct HarnessReport {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<HarnessUpdatePolicy>,
    #[serde(default)]
    pub versions: HarnessVersions,
    #[serde(default)]
    pub outcomes: Vec<HarnessOutcome>,
    /// A worker pod: the harness comes from the image and is never updated in place.
    #[serde(default)]
    pub managed_by_image: bool,
    /// Last codex sandbox probe on this machine. `None` from a daemon that does
    /// not probe, or before the first probe has run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_sandbox: Option<CodexSandbox>,
}

impl HarnessReport {
    /// Whether the daemon must be sent `effective`. A daemon holding nothing
    /// is treated as holding the default (off), so an absent setting sends
    /// nothing at all.
    #[must_use]
    pub fn needs_policy(&self, effective: &HarnessUpdatePolicy) -> bool {
        let held = self.policy.clone().unwrap_or_default().normalized();
        held != *effective
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(hours: u32) -> HarnessUpdatePolicy {
        HarnessUpdatePolicy { enabled: true, interval_hours: hours, ..Default::default() }
    }

    #[test]
    fn default_is_off() {
        let p = HarnessUpdatePolicy::resolve(None, None);
        assert!(!p.enabled);
        assert!(!p.covers(HARNESS_CODEX));
    }

    #[test]
    fn machine_override_beats_instance_default() {
        let off = HarnessUpdatePolicy::default();
        assert!(HarnessUpdatePolicy::resolve(Some(&on(24)), None).enabled);
        assert!(!HarnessUpdatePolicy::resolve(Some(&on(24)), Some(&off)).enabled);
        assert_eq!(HarnessUpdatePolicy::resolve(Some(&off), Some(&on(6))).interval_hours, 6);
    }

    #[test]
    fn normalization_clamps_and_filters() {
        let p = HarnessUpdatePolicy {
            enabled: true,
            interval_hours: 0,
            harnesses: vec!["codex".into(), "opencode".into(), "codex".into()],
        }
        .normalized();
        assert_eq!(p.interval_hours, MIN_INTERVAL_HOURS);
        assert_eq!(p.harnesses, vec!["codex".to_owned()]);
    }

    #[test]
    fn partial_json_fills_defaults() {
        let p: HarnessUpdatePolicy = serde_json::from_str(r#"{"enabled":true}"#).unwrap();
        assert_eq!(p, on(DEFAULT_INTERVAL_HOURS));
    }

    /// The tag and the variant names are the wire contract the webui binding
    /// matches on.
    #[test]
    fn the_sandbox_verdict_round_trips_on_its_state_tag() {
        let denied = CodexSandbox::UsernsDenied { detail: "bwrap: loopback".to_owned() };
        let json = serde_json::to_string(&denied).unwrap();
        assert!(json.contains(r#""state":"userns_denied""#), "{json}");
        assert_eq!(serde_json::from_str::<CodexSandbox>(&json).unwrap(), denied);
        assert_eq!(serde_json::to_string(&CodexSandbox::Ok).unwrap(), r#"{"state":"ok"}"#);

        assert!(CodexSandbox::Ok.is_ok());
        assert!(CodexSandbox::Ok.detail().is_none());
        assert_eq!(denied.detail(), Some("bwrap: loopback"));
        assert!(!CodexSandbox::Unknown { detail: "x".to_owned() }.is_ok());
    }

    /// A daemon too old to probe sends no field, and the report must still parse.
    #[test]
    fn a_report_without_a_sandbox_verdict_still_parses() {
        let report: HarnessReport =
            serde_json::from_str(r#"{"versions":{},"outcomes":[]}"#).unwrap();
        assert!(report.codex_sandbox.is_none());
        assert!(!serde_json::to_string(&report).unwrap().contains("codex_sandbox"));
    }

    #[test]
    fn versions_are_a_map_keyed_by_adapter_id() {
        let mut versions = HarnessVersions::default();
        versions.insert("gemini", HarnessVersion { cli: Some("0.62.0".into()), daemon: None });
        versions.insert(
            HARNESS_CODEX,
            HarnessVersion { cli: Some("0.155.0".into()), daemon: Some("0.155.0".into()) },
        );
        let json = serde_json::to_string(&versions).unwrap();
        assert_eq!(
            json,
            r#"{"codex":{"cli":"0.155.0","daemon":"0.155.0"},"gemini":{"cli":"0.62.0"}}"#
        );
        let back: HarnessVersions = serde_json::from_str(&json).unwrap();
        assert_eq!(back, versions);
        assert_eq!(back.get("gemini").and_then(|v| v.cli.as_deref()), Some("0.62.0"));
        assert_eq!(back.iter().map(|(k, _)| k).collect::<Vec<_>>(), ["codex", "gemini"]);
        assert!(HarnessVersions::default().is_empty());
    }

    /// A daemon from before the map sends `claude_code` and `codex` as fixed
    /// fields, `null` when unknown.
    #[test]
    fn legacy_fixed_fields_decode_under_their_adapter_ids() {
        let legacy = r#"{"claude_code":{"cli":"2.1.280","daemon":"2.1.279"},"codex":null}"#;
        let versions: HarnessVersions = serde_json::from_str(legacy).unwrap();
        assert_eq!(
            versions.get(HARNESS_CLAUDE_CODE).and_then(|v| v.daemon.as_deref()),
            Some("2.1.279")
        );
        assert!(versions.get("claude_code").is_none());
        assert!(versions.get(HARNESS_CODEX).is_none());
        let report: HarnessReport =
            serde_json::from_str(r#"{"versions":{"claude_code":{"cli":"2.1.280"}},"outcomes":[]}"#)
                .unwrap();
        assert_eq!(
            report.versions.get(HARNESS_CLAUDE_CODE).and_then(|v| v.cli.as_deref()),
            Some("2.1.280")
        );
    }

    #[test]
    fn absent_setting_never_needs_a_push() {
        let report = HarnessReport::default();
        assert!(!report.needs_policy(&HarnessUpdatePolicy::resolve(None, None)));
        assert!(report.needs_policy(&on(24)));
        let held = HarnessReport { policy: Some(on(24)), ..Default::default() };
        assert!(!held.needs_policy(&on(24)));
    }
}
