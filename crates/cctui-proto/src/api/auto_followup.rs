use serde::{Deserialize, Serialize};
#[cfg(feature = "ts")]
use ts_rs::TS;

pub const DEFAULT_THRESHOLD_TOKENS: u64 = 150_000;
pub const CLAUDE_WINDOW: u64 = 200_000;
pub const CLAUDE_1M_WINDOW: u64 = 1_000_000;
pub const CODEX_WINDOW: u64 = 272_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum ThresholdUnit {
    Tokens,
    Percent,
}

/// `percent`: 1 to 100 of the context window; `tokens`: above 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct AutoFollowupThreshold {
    pub unit: ThresholdUnit,
    pub value: u64,
}

impl AutoFollowupThreshold {
    #[must_use]
    pub const fn valid(self) -> bool {
        match self.unit {
            ThresholdUnit::Tokens => self.value > 0,
            ThresholdUnit::Percent => matches!(self.value, 1..=100),
        }
    }

    /// The threshold in tokens; `None` for a percent of an unknown window.
    #[must_use]
    pub const fn tokens(self, window: Option<u64>) -> Option<u64> {
        match (self.unit, window) {
            (ThresholdUnit::Tokens, _) => Some(self.value),
            (ThresholdUnit::Percent, Some(window)) => Some(window.saturating_mul(self.value) / 100),
            (ThresholdUnit::Percent, None) => None,
        }
    }
}

impl Default for AutoFollowupThreshold {
    fn default() -> Self {
        Self { unit: ThresholdUnit::Tokens, value: DEFAULT_THRESHOLD_TOKENS }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct AutoFollowupSetting {
    pub enabled: bool,
    pub threshold: AutoFollowupThreshold,
}

impl AutoFollowupSetting {
    #[must_use]
    pub const fn with(self, over: Option<&AutoFollowupOverride>) -> Self {
        let Some(over) = over else { return self };
        Self {
            enabled: match over.enabled {
                Some(enabled) => enabled,
                None => self.enabled,
            },
            threshold: match over.threshold {
                Some(threshold) => threshold,
                None => self.threshold,
            },
        }
    }
}

/// `null` fields inherit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct AutoFollowupOverride {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub threshold: Option<AutoFollowupThreshold>,
}

impl AutoFollowupOverride {
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.enabled.is_none() && self.threshold.is_none()
    }

    #[must_use]
    pub const fn valid(&self) -> bool {
        match self.threshold {
            Some(threshold) => threshold.valid(),
            None => true,
        }
    }
}

/// `never` is "Send here".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum SendFollowup {
    #[default]
    Auto,
    Never,
}

impl SendFollowup {
    #[must_use]
    #[allow(clippy::trivially_copy_pass_by_ref)]
    pub const fn is_auto(&self) -> bool {
        matches!(self, Self::Auto)
    }
}

#[must_use]
pub fn context_window(
    adapter_id: Option<&str>,
    model: Option<&str>,
    codex_window: Option<u64>,
) -> Option<u64> {
    let harness = crate::adapter::harness_for_adapter(adapter_id?)?;
    match harness.id.as_str() {
        "claude-code" if model.is_some_and(is_1m) => Some(CLAUDE_1M_WINDOW),
        "claude-code" => Some(CLAUDE_WINDOW),
        "codex" => Some(codex_window.filter(|w| *w > 0).unwrap_or(CODEX_WINDOW)),
        _ => None,
    }
}

fn is_1m(model: &str) -> bool {
    model.to_ascii_lowercase().contains("[1m]")
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn tokens(value: u64) -> AutoFollowupThreshold {
        AutoFollowupThreshold { unit: ThresholdUnit::Tokens, value }
    }

    #[test]
    fn the_default_setting_is_off_at_150k_tokens() {
        let setting = AutoFollowupSetting::default();
        assert!(!setting.enabled);
        assert_eq!(setting.threshold, tokens(150_000));
        assert_eq!(
            serde_json::to_value(setting).unwrap(),
            serde_json::json!({"enabled": false, "threshold": {"unit": "tokens", "value": 150_000}})
        );
    }

    #[test]
    fn thresholds_validate_their_range() {
        assert!(tokens(1).valid());
        assert!(!tokens(0).valid());
        let pct = |value| AutoFollowupThreshold { unit: ThresholdUnit::Percent, value };
        assert!(pct(1).valid() && pct(100).valid());
        assert!(!pct(0).valid() && !pct(101).valid());
        assert_eq!(pct(80).tokens(Some(200_000)), Some(160_000));
        assert_eq!(pct(80).tokens(None), None);
        assert_eq!(tokens(150_000).tokens(None), Some(150_000));
    }

    #[test]
    fn an_override_wins_field_by_field() {
        let base = AutoFollowupSetting { enabled: true, threshold: tokens(150_000) };
        let over = AutoFollowupOverride { enabled: None, threshold: Some(tokens(90_000)) };
        assert_eq!(
            base.with(Some(&over)),
            AutoFollowupSetting { enabled: true, threshold: tokens(90_000) }
        );
        let off = AutoFollowupOverride { enabled: Some(false), threshold: None };
        assert_eq!(base.with(Some(&off)), AutoFollowupSetting { enabled: false, ..base });
        assert_eq!(base.with(None), base);
    }

    #[test]
    fn an_override_reads_nulls_and_missing_fields_as_inherit() {
        let over: AutoFollowupOverride = serde_json::from_str("{}").unwrap();
        assert!(over.is_empty());
        let over: AutoFollowupOverride =
            serde_json::from_str(r#"{"enabled":true,"threshold":null}"#).unwrap();
        assert_eq!(over, AutoFollowupOverride { enabled: Some(true), threshold: None });
        assert_eq!(
            serde_json::to_value(over).unwrap(),
            serde_json::json!({"enabled": true, "threshold": null})
        );
    }

    #[test]
    fn send_followup_defaults_to_auto() {
        assert_eq!(SendFollowup::default(), SendFollowup::Auto);
        assert_eq!(serde_json::to_value(SendFollowup::Never).unwrap(), "never");
    }

    #[test]
    fn context_window_resolves_by_harness_and_model() {
        let claude = Some("claude-code");
        assert_eq!(context_window(claude, Some("opus[1m]"), None), Some(1_000_000));
        assert_eq!(context_window(claude, Some("claude-opus-5-5"), None), Some(200_000));
        assert_eq!(context_window(claude, None, None), Some(200_000));
        assert_eq!(context_window(Some("codex"), Some("gpt-5"), Some(400_000)), Some(400_000));
        assert_eq!(context_window(Some("codex-app-server"), None, None), Some(CODEX_WINDOW));
        assert_eq!(context_window(Some("opencode"), Some("x"), None), None);
        assert_eq!(context_window(None, Some("opus[1m]"), None), None);
    }
}
