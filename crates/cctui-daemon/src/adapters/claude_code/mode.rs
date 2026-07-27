//! Operating mode for the claude-code adapter.
//!
//! Generalizes the old binary `use_claude_daemon_path` switch into a real
//! enum so the adapter can dispatch to one of several drivers:
//!
//! - [`Mode::Bg`] — the default `claude daemon` control-socket client
//!   (`control::Driver`); spawns/observes long-lived background workers.
//! - [`Mode::Sdk`] — a stream-json driver over the Claude Agent SDK (stub
//!   until a later ticket).
//! - [`Mode::Oneshot`] — a stream-json driver over a single `claude
//!   --print --output-format stream-json` invocation (stub until a later
//!   ticket).
//! - [`Mode::Legacy`] — the line-delimited [`AdapterEvent`](cctui_proto::adapter::AdapterEvent)
//!   UDS listener kept until it is retired.
//!
//! Back-compat with the pre-enum config: the historical `mode` values
//! `"claude-daemon"` and `"legacy"` still resolve, and the
//! `CCTUI_ADAPTER_CLAUDE_DAEMON=0`/`false` env still forces [`Mode::Legacy`]
//! when `mode` is unset.

/// How the claude-code adapter runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    /// `claude daemon` control-socket client (default). Unchanged behavior.
    Bg,
    /// Stream-json driver over the Claude Agent SDK (stub).
    Sdk,
    /// Stream-json driver over a single `claude --print` invocation (stub).
    Oneshot,
    /// Legacy line-delimited UDS listener.
    Legacy,
}

impl Mode {
    /// Resolve the mode from the adapter's declarative config plus the
    /// `CCTUI_ADAPTER_CLAUDE_DAEMON` env override.
    ///
    /// Precedence: an explicit `config.mode` string wins; otherwise the env
    /// override picks between the default [`Mode::Bg`] and [`Mode::Legacy`].
    ///
    /// `mode` value mapping (case-insensitive):
    /// - `"bg"` / `"claude-daemon"` → [`Mode::Bg`]
    /// - `"sdk"` → [`Mode::Sdk`]
    /// - `"oneshot"` → [`Mode::Oneshot`]
    /// - `"legacy"` → [`Mode::Legacy`]
    /// - anything else → falls through to the env-default path
    pub(super) fn from_config(config: &serde_json::Value) -> Self {
        if let Some(raw) = config.get("mode").and_then(serde_json::Value::as_str) {
            match raw.trim().to_ascii_lowercase().as_str() {
                "bg" | "claude-daemon" => return Self::Bg,
                "sdk" => return Self::Sdk,
                "oneshot" => return Self::Oneshot,
                "legacy" => return Self::Legacy,
                // Unknown string: don't silently switch drivers — defer to the
                // env-default below (which is Bg unless explicitly disabled).
                _ => {}
            }
        }
        if matches!(std::env::var("CCTUI_ADAPTER_CLAUDE_DAEMON").as_deref(), Ok("0" | "false")) {
            Self::Legacy
        } else {
            Self::Bg
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn config_mode_maps_each_value() {
        assert_eq!(Mode::from_config(&json!({"mode": "bg"})), Mode::Bg);
        assert_eq!(Mode::from_config(&json!({"mode": "claude-daemon"})), Mode::Bg);
        assert_eq!(Mode::from_config(&json!({"mode": "sdk"})), Mode::Sdk);
        assert_eq!(Mode::from_config(&json!({"mode": "oneshot"})), Mode::Oneshot);
        assert_eq!(Mode::from_config(&json!({"mode": "legacy"})), Mode::Legacy);
    }

    #[test]
    fn config_mode_is_case_insensitive_and_trimmed() {
        assert_eq!(Mode::from_config(&json!({"mode": "  ONESHOT "})), Mode::Oneshot);
        assert_eq!(Mode::from_config(&json!({"mode": "Legacy"})), Mode::Legacy);
    }

    #[test]
    fn unknown_mode_falls_through_to_default() {
        // No env override set in this test process by default → Bg.
        assert_eq!(Mode::from_config(&json!({"mode": "wat"})), Mode::Bg);
    }

    #[test]
    fn absent_mode_defaults_to_bg() {
        assert_eq!(Mode::from_config(&json!({})), Mode::Bg);
        assert_eq!(Mode::from_config(&serde_json::Value::Null), Mode::Bg);
    }
}
