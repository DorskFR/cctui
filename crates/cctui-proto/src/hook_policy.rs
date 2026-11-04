//! Harness-neutral hook policy: rules cctui owns, converted per harness into
//! its native hook configuration at launch.

use serde::{Deserialize, Serialize};
#[cfg(feature = "ts")]
use ts_rs::TS;

pub const MAX_COMMAND_BYTES: usize = 4096;
pub const MAX_RULES: usize = 64;
pub const MAX_TIMEOUT_SECS: u32 = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub enum HookEvent {
    SessionStart,
    UserPromptSubmit,
    PreToolUse,
    PostToolUse,
    Stop,
}

impl HookEvent {
    #[must_use]
    pub const fn takes_tool(self) -> bool {
        matches!(self, Self::PreToolUse | Self::PostToolUse)
    }
}

/// A family of tools, named independently of any harness's tool names.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum ToolClass {
    Shell,
    FileEdit,
    FileWrite,
    Subagent,
    #[default]
    Any,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct HookRule {
    pub event: HookEvent,
    /// Only meaningful for tool events; ignored elsewhere.
    #[serde(default)]
    pub tool: ToolClass,
    /// Run through the harness's shell; exit 2 blocks where the event allows.
    pub command: String,
    #[serde(default)]
    pub timeout_secs: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct HookPolicy {
    #[serde(default)]
    pub rules: Vec<HookRule>,
}

/// Whether the session's harness took the policy at launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum HookPolicyStatus {
    Applied,
    /// The harness has no hook seam: the session runs unguarded by hooks.
    Unsupported,
}

/// `GET /sessions/{id}/hook-policy`: the rules sent with the session's launch.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct SessionHookPolicy {
    #[serde(default)]
    pub rules: Vec<HookRule>,
    /// `None` when the launch carried no rules.
    #[serde(default)]
    pub status: Option<HookPolicyStatus>,
}

/// Harnesses whose daemon adapter converts a [`HookPolicy`] into native hooks.
#[must_use]
pub fn supports_hook_policy(adapter_id: &str) -> bool {
    adapter_id == "claude-code"
}

/// Trim commands, drop blank ones and duplicates, and enforce the limits.
///
/// # Errors
/// A rule over a limit, with a human-readable reason.
pub fn normalize_rules(rules: Vec<HookRule>) -> Result<Vec<HookRule>, String> {
    let mut out: Vec<HookRule> = Vec::with_capacity(rules.len());
    for rule in rules {
        let command = rule.command.trim().to_owned();
        if command.is_empty() {
            continue;
        }
        if command.len() > MAX_COMMAND_BYTES {
            return Err(format!("hook command longer than {MAX_COMMAND_BYTES} bytes"));
        }
        if rule.timeout_secs.is_some_and(|t| t == 0 || t > MAX_TIMEOUT_SECS) {
            return Err(format!("hook timeout must be 1..={MAX_TIMEOUT_SECS} seconds"));
        }
        let tool = if rule.event.takes_tool() { rule.tool } else { ToolClass::Any };
        let rule = HookRule { event: rule.event, tool, command, timeout_secs: rule.timeout_secs };
        if !out.contains(&rule) {
            out.push(rule);
        }
    }
    if out.len() > MAX_RULES {
        return Err(format!("at most {MAX_RULES} hook rules"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(event: HookEvent, tool: ToolClass, command: &str) -> HookRule {
        HookRule { event, tool, command: command.into(), timeout_secs: None }
    }

    #[test]
    fn rules_use_harness_event_names_and_snake_case_tool_classes() {
        let r = HookRule {
            timeout_secs: Some(9),
            ..rule(HookEvent::PreToolUse, ToolClass::FileEdit, "x")
        };
        let json = serde_json::to_value(&r).unwrap();
        let want = serde_json::json!({
            "event": "PreToolUse", "tool": "file_edit", "command": "x", "timeout_secs": 9,
        });
        assert_eq!(json, want);
        assert_eq!(serde_json::from_value::<HookRule>(json).unwrap(), r);
    }

    #[test]
    fn an_absent_tool_means_any() {
        let r: HookRule = serde_json::from_str(r#"{"event":"Stop","command":"y"}"#).unwrap();
        assert_eq!(r, rule(HookEvent::Stop, ToolClass::Any, "y"));
    }

    #[test]
    fn normalizing_trims_dedupes_and_drops_tool_on_toolless_events() {
        let got = normalize_rules(vec![
            rule(HookEvent::PreToolUse, ToolClass::Shell, "  guard  "),
            rule(HookEvent::PreToolUse, ToolClass::Shell, "guard"),
            rule(HookEvent::Stop, ToolClass::Shell, "done"),
            rule(HookEvent::Stop, ToolClass::Any, "   "),
        ])
        .unwrap();
        assert_eq!(
            got,
            vec![
                rule(HookEvent::PreToolUse, ToolClass::Shell, "guard"),
                rule(HookEvent::Stop, ToolClass::Any, "done"),
            ]
        );
    }

    #[test]
    fn normalizing_rejects_out_of_range_timeouts() {
        let zero = HookRule { timeout_secs: Some(0), ..rule(HookEvent::Stop, ToolClass::Any, "x") };
        let huge = HookRule {
            timeout_secs: Some(MAX_TIMEOUT_SECS + 1),
            ..rule(HookEvent::Stop, ToolClass::Any, "x")
        };
        assert!(normalize_rules(vec![zero]).is_err());
        assert!(normalize_rules(vec![huge]).is_err());
    }

    #[test]
    fn only_claude_code_takes_a_hook_policy_today() {
        assert!(supports_hook_policy("claude-code"));
        assert!(!supports_hook_policy("codex"));
        assert!(!supports_hook_policy("opencode"));
    }
}
