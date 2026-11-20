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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowSource {
    Known,
    Observed,
    Assumed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextWindow {
    pub tokens: u64,
    pub source: WindowSource,
}

impl ContextWindow {
    /// A percent threshold never measures against an assumed window.
    #[must_use]
    pub const fn for_percent(self) -> Option<u64> {
        match self.source {
            WindowSource::Assumed => None,
            WindowSource::Known | WindowSource::Observed => Some(self.tokens),
        }
    }
}

/// `reported_model` is what the harness reports, which drops a `[1m]` suffix;
/// `launched_model` is what the session was launched with.
#[derive(Debug, Clone, Copy, Default)]
pub struct WindowHints<'a> {
    pub reported_model: Option<&'a str>,
    pub launched_model: Option<&'a str>,
    pub codex_window: Option<u64>,
    pub observed_tokens: Option<u64>,
}

#[must_use]
pub fn context_window(adapter_id: Option<&str>, hints: &WindowHints<'_>) -> Option<ContextWindow> {
    let harness = crate::adapter::harness_for_adapter(adapter_id?)?;
    let (tokens, source) = match harness.id.as_str() {
        "claude-code" => claude_window(hints),
        "codex" => {
            (hints.codex_window.filter(|w| *w > 0).unwrap_or(CODEX_WINDOW), WindowSource::Known)
        }
        _ => return None,
    };
    Some(ContextWindow { tokens, source })
}

fn claude_window(hints: &WindowHints<'_>) -> (u64, WindowSource) {
    if hints.launched_model.is_some_and(is_1m) || hints.reported_model.is_some_and(is_1m) {
        return (CLAUDE_1M_WINDOW, WindowSource::Known);
    }
    if hints.observed_tokens.is_some_and(|t| t > CLAUDE_WINDOW) {
        return (CLAUDE_1M_WINDOW, WindowSource::Observed);
    }
    (CLAUDE_WINDOW, WindowSource::Assumed)
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
        let window = |adapter, hints: WindowHints<'_>| {
            context_window(Some(adapter), &hints).map(|w| (w.tokens, w.source))
        };
        let reported = |m| WindowHints { reported_model: Some(m), ..WindowHints::default() };
        let launched = |m| WindowHints { launched_model: Some(m), ..WindowHints::default() };
        let known_1m = Some((1_000_000, WindowSource::Known));
        assert_eq!(window("claude-code", reported("opus[1m]")), known_1m);
        assert_eq!(window("claude-code", launched("claude-opus-5-5[1m]")), known_1m);
        assert_eq!(
            window("claude-code", launched("claude-opus-5-5")),
            Some((200_000, WindowSource::Assumed)),
            "a plain launch can still run 1M"
        );
        let observed =
            WindowHints { observed_tokens: Some(200_001), ..reported("claude-opus-5-5") };
        assert_eq!(window("claude-code", observed), Some((1_000_000, WindowSource::Observed)));
        let at_edge = WindowHints { observed_tokens: Some(200_000), ..WindowHints::default() };
        assert_eq!(window("claude-code", at_edge), Some((200_000, WindowSource::Assumed)));
        let codex = WindowHints { codex_window: Some(400_000), ..WindowHints::default() };
        assert_eq!(window("codex", codex), Some((400_000, WindowSource::Known)));
        let default_codex = window("codex-app-server", WindowHints::default());
        assert_eq!(default_codex.map(|w| w.0), Some(CODEX_WINDOW));
        assert_eq!(window("opencode", reported("x")), None);
        assert_eq!(context_window(None, &reported("opus[1m]")), None);
    }

    #[test]
    fn only_an_assumed_window_is_withheld_from_percent_thresholds() {
        let at = |source| ContextWindow { tokens: 200_000, source }.for_percent();
        assert_eq!(at(WindowSource::Known), Some(200_000));
        assert_eq!(at(WindowSource::Observed), Some(200_000));
        assert_eq!(at(WindowSource::Assumed), None);
    }
}
